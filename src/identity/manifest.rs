use std::collections::BTreeSet;
use std::fmt::{self, Display, Formatter};
use std::str::FromStr;

use crate::base_types::{BaseTypeCapability, BaseTypeId};
use crate::error::{SimardError, SimardResult};
use crate::prompt_assets::PromptAssetRef;

use super::{ManifestContract, MemoryPolicy, OperatingMode};

/// One identity-declared seed goal. Mirrors the shape of a
/// [`crate::goal_curation::DEFAULT_SEED_GOALS`] tuple
/// `(priority, title, description, Option<repo>)` so identity seeding and
/// default seeding stay a single shape (#3125).
///
/// When an identity declares a non-empty `seed_goals` list it OVERRIDES
/// Simard's baked-in `DEFAULT_SEED_GOALS` at the OODA cold-start seeding site;
/// an empty list falls through to the defaults, so Simard herself is unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SeedGoal {
    pub priority: u32,
    pub title: String,
    pub description: String,
    /// Target-repo slug. `None` means the identity's own repo; a slug scopes the
    /// goal to an ecosystem/target repo, exactly like `ActiveGoal.repo`.
    pub repo: Option<String>,
    /// Declares this a standing/perpetual goal (issue #4927). A standing seed
    /// produces a goal that reads as
    /// [`crate::goal_curation::ActiveGoal::is_perpetual`], so the no-progress
    /// breaker's `!is_perpetual()` exemption applies and the goal is never
    /// re-parked or issue-filed for lack of convergence. Additive and
    /// defaulting `false`, so every existing seed goal stays
    /// convergence-required exactly as before.
    pub standing: bool,
}

impl SeedGoal {
    pub fn new(
        priority: u32,
        title: impl Into<String>,
        description: impl Into<String>,
        repo: Option<String>,
    ) -> Self {
        Self {
            priority,
            title: title.into(),
            description: description.into(),
            repo,
            standing: false,
        }
    }

    /// Builder: declare this seed a standing/perpetual goal (issue #4927).
    /// Purely declarative — it flips the flag and never touches the
    /// description; the standing marker is applied later at the seed→
    /// [`crate::goal_curation::ActiveGoal`] conversion so
    /// [`crate::goal_curation::ActiveGoal::is_perpetual`] stays the single
    /// source of truth.
    #[must_use]
    pub fn standing(mut self) -> Self {
        self.standing = true;
        self
    }
}

/// The write-authority posture of an identity — the read-only switch that the
/// observe-only Act phase keys off (#3125, reusing the #3067 posture concept).
///
/// `Full` is the historical, default behaviour so Simard and any identity that
/// omits `[identities.authority]` are unchanged.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WritePosture {
    /// Bounded observer: reads and reasons, never writes anywhere.
    ReadOnly,
    /// Writes only to an explicit allowlist of repos.
    ScopedWrite,
    /// Historical behaviour — no posture restriction beyond the existing guards.
    #[default]
    Full,
}

impl WritePosture {
    /// Whether this posture is a definitively *writing* posture. `Full` and
    /// `ScopedWrite` write; `ReadOnly` does not. This is the pure kernel of the
    /// deterministic spawn rail — see
    /// [`crate::ooda_actions::advance_goal::spawn::posture_permits_spawn`].
    pub fn permits_writes(self) -> bool {
        matches!(self, Self::Full | Self::ScopedWrite)
    }
}

impl Display for WritePosture {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::ReadOnly => "read-only",
            Self::ScopedWrite => "scoped-write",
            Self::Full => "full",
        };
        f.write_str(label)
    }
}

impl FromStr for WritePosture {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "read-only" => Ok(Self::ReadOnly),
            "scoped-write" => Ok(Self::ScopedWrite),
            "full" => Ok(Self::Full),
            other => Err(format!(
                "unknown write posture: '{other}' (expected read-only | scoped-write | full)"
            )),
        }
    }
}

/// The typed write-authority contract on an identity. `Default` is `Full` with
/// every `allow_*` true, so built-in identities and TOML identities that omit
/// `[identities.authority]` behave exactly as before this feature.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityAuthority {
    pub posture: WritePosture,
    pub allowed_write_repos: Vec<String>,
    pub allow_git_push: bool,
    pub allow_ado_writes: bool,
    pub allow_github_writes: bool,
}

impl Default for IdentityAuthority {
    fn default() -> Self {
        Self {
            posture: WritePosture::Full,
            allowed_write_repos: Vec::new(),
            allow_git_push: true,
            allow_ado_writes: true,
            allow_github_writes: true,
        }
    }
}

impl IdentityAuthority {
    /// A `read-only` authority with every write path denied. The canonical
    /// bounded-observer posture; also the fail-closed value the daemon installs
    /// when an identity is named but its posture cannot be resolved.
    pub fn read_only() -> Self {
        Self {
            posture: WritePosture::ReadOnly,
            allowed_write_repos: Vec::new(),
            allow_git_push: false,
            allow_ado_writes: false,
            allow_github_writes: false,
        }
    }

    /// Whether this authority permits dispatching a write-bearing engineer.
    /// `ReadOnly` never does; `Full`/`ScopedWrite` do.
    pub fn permits_spawn(&self) -> bool {
        self.posture.permits_writes()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdentityManifest {
    pub name: String,
    pub version: String,
    pub prompt_assets: Vec<PromptAssetRef>,
    pub components: Vec<String>,
    pub supported_base_types: Vec<BaseTypeId>,
    pub required_capabilities: BTreeSet<BaseTypeCapability>,
    pub default_mode: OperatingMode,
    pub memory_policy: MemoryPolicy,
    pub contract: ManifestContract,
    /// Identity-declared seed goals. Empty => use `DEFAULT_SEED_GOALS` (#3125).
    pub seed_goals: Vec<SeedGoal>,
    /// Target repo set for goals/observations. Empty => union of
    /// `seed_goals[].repo` (#3125). Never scopes to `rysweet/Simard`.
    pub target_repos: Vec<String>,
    /// Write-authority posture (the read-only switch). Default `Full`.
    pub authority: IdentityAuthority,
}

impl IdentityManifest {
    #[expect(
        clippy::too_many_arguments,
        reason = "identity manifests are explicit contract values with distinct fields"
    )]
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        prompt_assets: Vec<PromptAssetRef>,
        supported_base_types: Vec<BaseTypeId>,
        required_capabilities: BTreeSet<BaseTypeCapability>,
        default_mode: OperatingMode,
        memory_policy: MemoryPolicy,
        contract: ManifestContract,
    ) -> SimardResult<Self> {
        memory_policy.validate()?;

        Ok(Self {
            name: name.into(),
            version: version.into(),
            prompt_assets,
            components: Vec::new(),
            supported_base_types,
            required_capabilities,
            default_mode,
            memory_policy,
            contract,
            seed_goals: Vec::new(),
            target_repos: Vec::new(),
            authority: IdentityAuthority::default(),
        })
    }

    /// Attach identity-declared seed goals (override for `DEFAULT_SEED_GOALS`).
    /// Additive builder: the default is an empty list (Simard's defaults).
    #[must_use]
    pub fn with_seed_goals(mut self, seed_goals: Vec<SeedGoal>) -> Self {
        self.seed_goals = seed_goals;
        self
    }

    /// Attach an explicit target-repo scope. Additive builder; the default is
    /// empty (resolved as the union of `seed_goals[].repo`).
    #[must_use]
    pub fn with_target_repos(mut self, target_repos: Vec<String>) -> Self {
        self.target_repos = target_repos;
        self
    }

    /// Attach a write-authority posture. Additive builder; the default is
    /// [`IdentityAuthority::default`] (`Full`), so Simard is unchanged.
    #[must_use]
    pub fn with_authority(mut self, authority: IdentityAuthority) -> Self {
        self.authority = authority;
        self
    }

    /// The resolved target-repo set: the explicit `target_repos` when present,
    /// otherwise the de-duplicated union of the `repo` slugs on `seed_goals`.
    /// A read-only identity with an empty resolved set scopes to nothing and
    /// (fail-closed) never falls back to the daemon's own repo (#3125).
    pub fn resolved_target_repos(&self) -> Vec<String> {
        if !self.target_repos.is_empty() {
            return self.target_repos.clone();
        }
        let mut seen = BTreeSet::new();
        let mut union = Vec::new();
        for goal in &self.seed_goals {
            if let Some(repo) = &goal.repo
                && seen.insert(repo.clone())
            {
                union.push(repo.clone());
            }
        }
        union
    }

    pub fn with_components(
        mut self,
        components: impl IntoIterator<Item = impl Into<String>>,
    ) -> SimardResult<Self> {
        let mut seen = BTreeSet::new();
        let mut normalized = Vec::new();
        for component in components {
            let component = component.into().trim().to_string();
            if component.is_empty() {
                return Err(SimardError::InvalidIdentityComposition {
                    identity: self.name.clone(),
                    reason: "component identities cannot be empty".to_string(),
                });
            }
            if component == self.name {
                return Err(SimardError::InvalidIdentityComposition {
                    identity: self.name.clone(),
                    reason: "an identity cannot list itself as a component".to_string(),
                });
            }
            if !seen.insert(component.clone()) {
                return Err(SimardError::InvalidIdentityComposition {
                    identity: self.name.clone(),
                    reason: format!("duplicate component identity '{component}'"),
                });
            }
            normalized.push(component);
        }
        self.components = normalized;
        Ok(self)
    }

    pub fn supports_base_type(&self, base_type: &BaseTypeId) -> bool {
        self.supported_base_types
            .iter()
            .any(|candidate| candidate == base_type)
    }
}

/// Compose multiple identity manifests using precedence-based conflict
/// resolution. Index 0 in the input `Vec` is the highest-precedence manifest.
///
/// Delegates to [`crate::identity_precedence::PrecedenceResolver`].
pub fn compose_with_precedence(
    manifests: Vec<IdentityManifest>,
) -> crate::identity_precedence::ResolvedIdentity {
    crate::identity_precedence::PrecedenceResolver::new(manifests).resolve_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base_types::capability_set;
    use crate::metadata::{Freshness, Provenance};

    fn test_contract() -> ManifestContract {
        ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn identity_manifest_supports_base_type_check() {
        let manifest = IdentityManifest::new(
            "test-identity",
            "0.1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        assert!(manifest.supports_base_type(&BaseTypeId::new("local-harness")));
        assert!(!manifest.supports_base_type(&BaseTypeId::new("unknown")));
    }

    // --- IdentityManifest ---

    #[test]
    fn identity_manifest_new_rejects_project_writes_policy() {
        let policy = MemoryPolicy {
            allow_project_writes: true,
            summary_scope: crate::memory::MemoryScope::SessionSummary,
        };
        let err = IdentityManifest::new(
            "test",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            policy,
            test_contract(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::UnsupportedMemoryPolicy { .. }));
    }

    #[test]
    fn identity_manifest_with_components_success() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
        .with_components(["child-a", "child-b"])
        .unwrap();
        assert_eq!(manifest.components, vec!["child-a", "child-b"]);
    }

    #[test]
    fn identity_manifest_with_components_rejects_empty() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = manifest.with_components(["  "]).unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn identity_manifest_with_components_rejects_self_reference() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = manifest.with_components(["parent"]).unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn identity_manifest_with_components_rejects_duplicates() {
        let manifest = IdentityManifest::new(
            "parent",
            "1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        let err = manifest
            .with_components(["child-a", "child-a"])
            .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidIdentityComposition { .. }
        ));
    }

    #[test]
    fn supports_base_type_returns_false_for_nonexistent() {
        let manifest = IdentityManifest::new(
            "test",
            "1.0",
            vec![],
            vec![
                BaseTypeId::new("local-harness"),
                BaseTypeId::new("rusty-clawd"),
            ],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();
        assert!(manifest.supports_base_type(&BaseTypeId::new("local-harness")));
        assert!(manifest.supports_base_type(&BaseTypeId::new("rusty-clawd")));
        assert!(!manifest.supports_base_type(&BaseTypeId::new("nonexistent")));
    }

    // --- #3125: identity-scoped cognition fields ---

    fn base_manifest() -> IdentityManifest {
        IdentityManifest::new(
            "test-identity",
            "0.1.0",
            vec![],
            vec![BaseTypeId::new("local-harness")],
            capability_set([]),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
    }

    #[test]
    fn manifest_defaults_leave_simard_unchanged() {
        // A manifest built via `new` (as every built-in loader does) must have
        // NO identity seed goals, NO explicit target scope, and a `Full`
        // posture — this is the invariant that keeps Simard herself unchanged.
        let manifest = base_manifest();
        assert!(manifest.seed_goals.is_empty());
        assert!(manifest.target_repos.is_empty());
        assert_eq!(manifest.authority, IdentityAuthority::default());
        assert_eq!(manifest.authority.posture, WritePosture::Full);
        assert!(manifest.authority.permits_spawn());
        assert!(manifest.resolved_target_repos().is_empty());
    }

    #[test]
    fn write_posture_default_is_full() {
        assert_eq!(WritePosture::default(), WritePosture::Full);
    }

    #[test]
    fn write_posture_permits_writes_matrix() {
        assert!(WritePosture::Full.permits_writes());
        assert!(WritePosture::ScopedWrite.permits_writes());
        assert!(!WritePosture::ReadOnly.permits_writes());
    }

    #[test]
    fn write_posture_parses_and_displays_kebab_case() {
        assert_eq!(
            "read-only".parse::<WritePosture>().unwrap(),
            WritePosture::ReadOnly
        );
        assert_eq!(
            "scoped-write".parse::<WritePosture>().unwrap(),
            WritePosture::ScopedWrite
        );
        assert_eq!("full".parse::<WritePosture>().unwrap(), WritePosture::Full);
        assert!("read_write".parse::<WritePosture>().is_err());
        assert_eq!(WritePosture::ReadOnly.to_string(), "read-only");
        assert_eq!(WritePosture::ScopedWrite.to_string(), "scoped-write");
        assert_eq!(WritePosture::Full.to_string(), "full");
    }

    #[test]
    fn identity_authority_default_is_full_all_allowed() {
        let a = IdentityAuthority::default();
        assert_eq!(a.posture, WritePosture::Full);
        assert!(a.allow_git_push);
        assert!(a.allow_ado_writes);
        assert!(a.allow_github_writes);
        assert!(a.allowed_write_repos.is_empty());
        assert!(a.permits_spawn());
    }

    #[test]
    fn identity_authority_read_only_denies_all() {
        let a = IdentityAuthority::read_only();
        assert_eq!(a.posture, WritePosture::ReadOnly);
        assert!(!a.allow_git_push);
        assert!(!a.allow_ado_writes);
        assert!(!a.allow_github_writes);
        assert!(!a.permits_spawn());
    }

    #[test]
    fn with_seed_goals_and_authority_are_additive() {
        let manifest = base_manifest()
            .with_seed_goals(vec![
                SeedGoal::new(
                    1,
                    "Observe hyenas repo health",
                    "OBSERVE ONLY",
                    Some("hyenas".into()),
                ),
                SeedGoal::new(
                    2,
                    "Articulate repo-hygiene backlog",
                    "propose goals",
                    Some("hyenas".into()),
                ),
            ])
            .with_authority(IdentityAuthority::read_only());
        assert_eq!(manifest.seed_goals.len(), 2);
        assert_eq!(manifest.authority.posture, WritePosture::ReadOnly);
        // target scope falls back to the union of the seed goals' repos.
        assert_eq!(manifest.resolved_target_repos(), vec!["hyenas".to_string()]);
    }

    #[test]
    fn resolved_target_repos_prefers_explicit_over_union() {
        let manifest = base_manifest()
            .with_seed_goals(vec![SeedGoal::new(1, "g", "d", Some("hyenas".into()))])
            .with_target_repos(vec!["explicit-target".into()]);
        assert_eq!(
            manifest.resolved_target_repos(),
            vec!["explicit-target".to_string()]
        );
    }

    #[test]
    fn resolved_target_repos_dedups_union() {
        let manifest = base_manifest().with_seed_goals(vec![
            SeedGoal::new(1, "a", "d", Some("hyenas".into())),
            SeedGoal::new(2, "b", "d", Some("hyenas".into())),
            SeedGoal::new(3, "c", "d", None),
        ]);
        assert_eq!(manifest.resolved_target_repos(), vec!["hyenas".to_string()]);
    }
}
