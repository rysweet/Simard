mod build;
mod persistence;
mod types;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_build;

// Re-export all public items so `crate::review::X` still works.
pub use build::build_review_artifact;
pub use persistence::{
    latest_review_artifact, load_review_artifact, persist_review_artifact, render_review_text,
    review_artifacts_dir,
};
pub use types::{
    ImprovementProposal, ReviewArtifact, ReviewEvidenceSummary, ReviewRequest, ReviewSignal,
    ReviewTargetKind,
};
