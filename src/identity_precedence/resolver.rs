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
                .map(|m| m.name.as_str())
                .unwrap_or("unknown");

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
