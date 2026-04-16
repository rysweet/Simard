//! Precedence resolver and resolved-identity types.

use std::collections::{BTreeMap, BTreeSet};

use crate::base_types::{BaseTypeCapability, BaseTypeId};
use crate::identity::IdentityManifest;
use crate::prompt_assets::{PromptAssetId, PromptAssetRef};

use super::conflict::ConflictLog;

/// The fully resolved identity produced by merging multiple manifests.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedIdentity {
    pub prompt_assets: Vec<PromptAssetRef>,
    pub capabilities: BTreeSet<BaseTypeCapability>,
    pub base_types: Vec<BaseTypeId>,
    pub metadata: BTreeMap<String, String>,
    pub conflict_log: ConflictLog,
}

impl Default for ResolvedIdentity {
    fn default() -> Self {
        Self {
            prompt_assets: Vec::new(),
            capabilities: BTreeSet::new(),
            base_types: Vec::new(),
            metadata: BTreeMap::new(),
            conflict_log: ConflictLog::new(),
        }
    }
}

/// Resolves conflicts across a precedence-ordered list of identity manifests.
///
/// Index 0 in the input is highest-precedence (wins conflicts).
#[derive(Debug)]
pub struct PrecedenceResolver {
    manifests: Vec<IdentityManifest>,
}

impl PrecedenceResolver {
    pub fn new(manifests: Vec<IdentityManifest>) -> Self {
        Self { manifests }
    }

    /// Resolve prompt assets: higher-precedence identity wins on `PromptAssetId` collision.
    pub fn resolve_prompt_assets(&self) -> (Vec<PromptAssetRef>, ConflictLog) {
        let mut log = ConflictLog::new();
        // Track which identity claimed each asset id first (= highest precedence).
        let mut seen: BTreeMap<PromptAssetId, (PromptAssetRef, String)> = BTreeMap::new();

        for manifest in &self.manifests {
            for asset in &manifest.prompt_assets {
                if let Some((_, winner_name)) = seen.get(&asset.id) {
                    log.record(
                        "prompt_asset",
                        asset.id.as_str(),
                        winner_name.clone(),
                        &manifest.name,
                    );
                } else {
                    seen.insert(asset.id.clone(), (asset.clone(), manifest.name.clone()));
                }
            }
        }

        let assets = seen.into_values().map(|(asset, _)| asset).collect();
        (assets, log)
    }

    /// Resolve capabilities: union of all capabilities (BTreeSet deduplicates).
    pub fn resolve_capabilities(&self) -> BTreeSet<BaseTypeCapability> {
        let mut capabilities = BTreeSet::new();
        for manifest in &self.manifests {
            capabilities.extend(manifest.required_capabilities.iter().copied());
        }
        capabilities
    }

    /// Resolve base types: higher-precedence wins on `BaseTypeId` name collision.
    pub fn resolve_base_types(&self) -> (Vec<BaseTypeId>, ConflictLog) {
        let mut log = ConflictLog::new();
        let mut seen: BTreeMap<String, (BaseTypeId, String)> = BTreeMap::new();

        for manifest in &self.manifests {
            for base_type in &manifest.supported_base_types {
                let key = base_type.as_str().to_string();
                if let Some((_, winner_name)) = seen.get(&key) {
                    log.record("base_type", &key, winner_name.clone(), &manifest.name);
                } else {
                    seen.insert(key, (base_type.clone(), manifest.name.clone()));
                }
            }
        }

        let base_types = seen.into_values().map(|(bt, _)| bt).collect();
        (base_types, log)
    }

    /// Resolve metadata: higher-precedence wins on key collision.
    pub fn resolve_metadata(
        &self,
        per_manifest_metadata: &[BTreeMap<String, String>],
    ) -> (BTreeMap<String, String>, ConflictLog) {
        let mut log = ConflictLog::new();
        let mut merged: BTreeMap<String, (String, String)> = BTreeMap::new();

        for (index, meta) in per_manifest_metadata.iter().enumerate() {
            let manifest_name = self
                .manifests
                .get(index)
                .map_or("unknown", |m| m.name.as_str());

            for (key, value) in meta {
                if let Some((_, winner_name)) = merged.get(key) {
                    log.record("metadata", key, winner_name.clone(), manifest_name);
                } else {
                    merged.insert(key.clone(), (value.clone(), manifest_name.to_string()));
                }
            }
        }

        let result = merged
            .into_iter()
            .map(|(key, (value, _))| (key, value))
            .collect();
        (result, log)
    }

    /// Resolve all fields into a single [`ResolvedIdentity`].
    pub fn resolve_all(&self) -> ResolvedIdentity {
        self.resolve_all_with_metadata(&[])
    }

    /// Resolve all fields, including external metadata maps.
    pub fn resolve_all_with_metadata(
        &self,
        per_manifest_metadata: &[BTreeMap<String, String>],
    ) -> ResolvedIdentity {
        if self.manifests.is_empty() {
            return ResolvedIdentity::default();
        }

        let (prompt_assets, mut conflict_log) = self.resolve_prompt_assets();
        let capabilities = self.resolve_capabilities();
        let (base_types, base_type_log) = self.resolve_base_types();
        let (metadata, metadata_log) = self.resolve_metadata(per_manifest_metadata);

        conflict_log.entries.extend(base_type_log.entries);
        conflict_log.entries.extend(metadata_log.entries);

        ResolvedIdentity {
            prompt_assets,
            capabilities,
            base_types,
            metadata,
            conflict_log,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ManifestContract, MemoryPolicy, OperatingMode};
    use crate::metadata::{Freshness, Provenance};

    fn test_contract() -> ManifestContract {
        ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["layer:test".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap()
    }

    fn make_manifest(name: &str) -> IdentityManifest {
        IdentityManifest::new(
            name,
            "1.0",
            Vec::new(),
            Vec::new(),
            BTreeSet::new(),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
    }

    fn manifest_with_assets(name: &str, asset_ids: &[&str]) -> IdentityManifest {
        let assets: Vec<PromptAssetRef> = asset_ids
            .iter()
            .map(|id| PromptAssetRef::new(*id, format!("{id}.md")))
            .collect();
        IdentityManifest::new(
            name,
            "1.0",
            assets,
            Vec::new(),
            BTreeSet::new(),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
    }

    fn manifest_with_base_types(name: &str, bt_names: &[&str]) -> IdentityManifest {
        let bts: Vec<BaseTypeId> = bt_names.iter().map(|n| BaseTypeId::new(*n)).collect();
        IdentityManifest::new(
            name,
            "1.0",
            Vec::new(),
            bts,
            BTreeSet::new(),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap()
    }

    // -- ResolvedIdentity default --

    #[test]
    fn resolved_identity_default_is_empty() {
        let ri = ResolvedIdentity::default();
        assert!(ri.prompt_assets.is_empty());
        assert!(ri.capabilities.is_empty());
        assert!(ri.base_types.is_empty());
        assert!(ri.metadata.is_empty());
        assert!(ri.conflict_log.entries.is_empty());
    }

    // -- PrecedenceResolver with empty manifests --

    #[test]
    fn resolve_all_with_empty_manifests_returns_default() {
        let resolver = PrecedenceResolver::new(vec![]);
        let result = resolver.resolve_all();
        assert_eq!(result, ResolvedIdentity::default());
    }

    // -- resolve_prompt_assets --

    #[test]
    fn resolve_prompt_assets_no_collision() {
        let m1 = manifest_with_assets("high", &["a"]);
        let m2 = manifest_with_assets("low", &["b"]);
        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let (assets, log) = resolver.resolve_prompt_assets();
        assert_eq!(assets.len(), 2);
        assert!(log.entries.is_empty());
    }

    #[test]
    fn resolve_prompt_assets_higher_precedence_wins_collision() {
        let m1 = manifest_with_assets("high", &["shared"]);
        let m2 = manifest_with_assets("low", &["shared"]);
        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let (assets, log) = resolver.resolve_prompt_assets();
        assert_eq!(assets.len(), 1);
        assert_eq!(log.entries.len(), 1);
        assert_eq!(log.entries[0].winner, "high");
        assert_eq!(log.entries[0].loser, "low");
    }

    // -- resolve_capabilities --

    #[test]
    fn resolve_capabilities_unions_all() {
        let mut caps1 = BTreeSet::new();
        caps1.insert(BaseTypeCapability::Memory);
        let mut caps2 = BTreeSet::new();
        caps2.insert(BaseTypeCapability::Evidence);
        caps2.insert(BaseTypeCapability::Memory);

        let mut m1 = make_manifest("a");
        m1.required_capabilities = caps1;
        let mut m2 = make_manifest("b");
        m2.required_capabilities = caps2;

        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let caps = resolver.resolve_capabilities();
        assert!(caps.contains(&BaseTypeCapability::Memory));
        assert!(caps.contains(&BaseTypeCapability::Evidence));
        assert_eq!(caps.len(), 2);
    }

    // -- resolve_base_types --

    #[test]
    fn resolve_base_types_no_collision() {
        let m1 = manifest_with_base_types("high", &["terminal"]);
        let m2 = manifest_with_base_types("low", &["copilot"]);
        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let (bts, log) = resolver.resolve_base_types();
        assert_eq!(bts.len(), 2);
        assert!(log.entries.is_empty());
    }

    #[test]
    fn resolve_base_types_higher_precedence_wins() {
        let m1 = manifest_with_base_types("high", &["shared"]);
        let m2 = manifest_with_base_types("low", &["shared"]);
        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let (bts, log) = resolver.resolve_base_types();
        assert_eq!(bts.len(), 1);
        assert_eq!(log.entries.len(), 1);
    }

    // -- resolve_metadata --

    #[test]
    fn resolve_metadata_merges_disjoint_keys() {
        let m1 = make_manifest("a");
        let m2 = make_manifest("b");
        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let meta1: BTreeMap<String, String> =
            [("k1".to_string(), "v1".to_string())].into_iter().collect();
        let meta2: BTreeMap<String, String> =
            [("k2".to_string(), "v2".to_string())].into_iter().collect();
        let (merged, log) = resolver.resolve_metadata(&[meta1, meta2]);
        assert_eq!(merged.len(), 2);
        assert!(log.entries.is_empty());
    }

    #[test]
    fn resolve_metadata_higher_precedence_wins_key_conflict() {
        let m1 = make_manifest("winner");
        let m2 = make_manifest("loser");
        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let meta1: BTreeMap<String, String> = [("key".to_string(), "winner-val".to_string())]
            .into_iter()
            .collect();
        let meta2: BTreeMap<String, String> = [("key".to_string(), "loser-val".to_string())]
            .into_iter()
            .collect();
        let (merged, log) = resolver.resolve_metadata(&[meta1, meta2]);
        assert_eq!(merged["key"], "winner-val");
        assert_eq!(log.entries.len(), 1);
    }

    // -- resolve_all --

    #[test]
    fn resolve_all_aggregates_conflict_logs() {
        let m1 = IdentityManifest::new(
            "high",
            "1.0",
            vec![PromptAssetRef::new("shared-asset", "a.md")],
            vec![BaseTypeId::new("shared-bt")],
            BTreeSet::new(),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();

        let m2 = IdentityManifest::new(
            "low",
            "1.0",
            vec![PromptAssetRef::new("shared-asset", "b.md")],
            vec![BaseTypeId::new("shared-bt")],
            BTreeSet::new(),
            OperatingMode::Engineer,
            MemoryPolicy::default(),
            test_contract(),
        )
        .unwrap();

        let resolver = PrecedenceResolver::new(vec![m1, m2]);
        let result = resolver.resolve_all();
        // One conflict from prompt_assets + one from base_types = 2.
        assert_eq!(result.conflict_log.entries.len(), 2);
    }
}
