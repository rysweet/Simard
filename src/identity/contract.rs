use std::collections::BTreeSet;

use crate::error::{SimardError, SimardResult};
use crate::metadata::{Freshness, Provenance};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManifestContract {
    pub entrypoint: String,
    pub composition: String,
    pub precedence: Vec<String>,
    pub provenance: Provenance,
    pub freshness: Freshness,
}

impl ManifestContract {
    pub fn new(
        entrypoint: impl Into<String>,
        composition: impl Into<String>,
        precedence: Vec<String>,
        provenance: Provenance,
        freshness: Freshness,
    ) -> SimardResult<Self> {
        let entrypoint = required_entrypoint(entrypoint.into())?;
        let composition = required_composition(composition.into())?;
        if precedence.is_empty() {
            return Err(SimardError::InvalidManifestContract {
                field: "precedence".to_string(),
                reason: "at least one precedence value is required".to_string(),
            });
        }
        let mut seen_precedence = BTreeSet::new();
        let precedence = precedence
            .into_iter()
            .map(|value| {
                let value = required_contract_field("precedence", value)?;
                if !value.contains(':') {
                    return Err(SimardError::InvalidManifestContract {
                        field: "precedence".to_string(),
                        reason: "precedence entries must look like 'key:value'".to_string(),
                    });
                }
                if !seen_precedence.insert(value.clone()) {
                    return Err(SimardError::InvalidManifestContract {
                        field: "precedence".to_string(),
                        reason: format!("duplicate precedence value '{value}'"),
                    });
                }
                Ok(value)
            })
            .collect::<SimardResult<Vec<_>>>()?;
        let provenance_source = required_provenance_source(required_contract_field(
            "provenance.source",
            provenance.source,
        )?)?;
        let provenance_locator = required_contract_field("provenance.locator", provenance.locator)?;

        Ok(Self {
            entrypoint,
            composition,
            precedence,
            provenance: Provenance::new(provenance_source, provenance_locator),
            freshness,
        })
    }

    pub fn with_freshness(&self, freshness: Freshness) -> Self {
        Self {
            entrypoint: self.entrypoint.clone(),
            composition: self.composition.clone(),
            precedence: self.precedence.clone(),
            provenance: self.provenance.clone(),
            freshness,
        }
    }
}

fn required_contract_field(field: &str, value: String) -> SimardResult<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(SimardError::InvalidManifestContract {
            field: field.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }
    Ok(trimmed.to_string())
}

fn required_entrypoint(value: String) -> SimardResult<String> {
    let entrypoint = required_contract_field("entrypoint", value)?;
    if !entrypoint.contains("::") {
        return Err(SimardError::InvalidManifestContract {
            field: "entrypoint".to_string(),
            reason: "expected a Rust-style module::function path".to_string(),
        });
    }
    if entrypoint == "inline-manifest" {
        return Err(SimardError::InvalidManifestContract {
            field: "entrypoint".to_string(),
            reason: "placeholder entrypoints are not allowed".to_string(),
        });
    }
    Ok(entrypoint)
}

fn required_composition(value: String) -> SimardResult<String> {
    let composition = required_contract_field("composition", value)?;
    if !composition.contains("->") {
        return Err(SimardError::InvalidManifestContract {
            field: "composition".to_string(),
            reason: "expected a 'component -> component' composition chain".to_string(),
        });
    }
    Ok(composition)
}

fn required_provenance_source(value: String) -> SimardResult<String> {
    if value == "inline" {
        return Err(SimardError::InvalidManifestContract {
            field: "provenance.source".to_string(),
            reason: "placeholder provenance sources are not allowed".to_string(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Freshness, Provenance};

    #[test]
    fn manifest_contract_requires_rust_style_entrypoint() {
        let err = ManifestContract::new(
            "no-colons",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "entrypoint"
        ));
    }

    #[test]
    fn manifest_contract_rejects_placeholder_entrypoint() {
        let err = ManifestContract::new(
            "inline-manifest",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_requires_composition_chain() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "no arrow",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "composition"
        ));
    }

    #[test]
    fn manifest_contract_rejects_empty_precedence() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec![],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "precedence"
        ));
    }

    #[test]
    fn manifest_contract_rejects_duplicate_precedence() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string(), "key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_rejects_inline_provenance() {
        let err = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("inline", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "provenance.source"
        ));
    }

    // --- ManifestContract ---

    #[test]
    fn manifest_contract_valid_construction() {
        let contract = ManifestContract::new(
            "module::function",
            "a -> b",
            vec!["identity:simard".to_string(), "base-type:local".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap();
        assert_eq!(contract.entrypoint, "module::function");
        assert_eq!(contract.composition, "a -> b");
        assert_eq!(contract.precedence.len(), 2);
    }

    #[test]
    fn manifest_contract_rejects_empty_entrypoint() {
        let err = ManifestContract::new(
            "",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "entrypoint"
        ));
    }

    #[test]
    fn manifest_contract_rejects_empty_composition() {
        let err = ManifestContract::new(
            "test::entry",
            "",
            vec!["key:value".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "composition"
        ));
    }

    #[test]
    fn manifest_contract_rejects_precedence_without_colon() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["no-colon-here".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            SimardError::InvalidManifestContract { field, .. } if field == "precedence"
        ));
    }

    #[test]
    fn manifest_contract_rejects_empty_precedence_value() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["  ".to_string()],
            Provenance::new("s", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_rejects_empty_provenance_source() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("", "l"),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_rejects_empty_provenance_locator() {
        let err = ManifestContract::new(
            "test::entry",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("src", ""),
            Freshness::now().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err, SimardError::InvalidManifestContract { .. }));
    }

    #[test]
    fn manifest_contract_with_freshness_preserves_other_fields() {
        let contract = ManifestContract::new(
            "test::entrypoint",
            "a -> b",
            vec!["key:value".to_string()],
            Provenance::new("test-source", "test-locator"),
            Freshness::now().unwrap(),
        )
        .unwrap();
        let new_freshness = Freshness::now().unwrap();
        let updated = contract.with_freshness(new_freshness);
        assert_eq!(updated.entrypoint, contract.entrypoint);
        assert_eq!(updated.composition, contract.composition);
        assert_eq!(updated.precedence, contract.precedence);
        assert_eq!(updated.freshness, new_freshness);
    }
}
