use crate::error::{SimardError, SimardResult};
use crate::stewardship::gh_client::IssueMutationTransport;
use crate::stewardship::mutation_store::{
    GitHubReservationDecision, MutationStore, ReservationDecision,
};
use crate::stewardship::types::{
    ArtifactProvenance, CycleId, GitHubMutationOutcome, GitHubMutationRequest,
    GitHubMutationResult, IssueMutation, IssueMutationIdentity, IssueMutationLimit,
    IssueMutationOutcome, IssueMutationRequest,
};

pub struct MutationGuard {
    store: MutationStore,
}

impl MutationGuard {
    pub fn new(store: MutationStore) -> Self {
        Self { store }
    }

    pub fn from_default_store() -> Self {
        Self::new(MutationStore::new(MutationStore::default_path()))
    }

    pub fn begin_cycle(
        &mut self,
        cycle_id: CycleId,
        limit: IssueMutationLimit,
    ) -> SimardResult<()> {
        self.store.begin_cycle(cycle_id, limit)
    }

    pub fn cycle_failure(&self, cycle_id: &CycleId) -> SimardResult<Option<String>> {
        self.store.cycle_failure(cycle_id)
    }

    pub(crate) fn execute(
        &mut self,
        cycle_id: &CycleId,
        request: &IssueMutationRequest,
        transport: &dyn IssueMutationTransport,
    ) -> SimardResult<IssueMutationOutcome> {
        request.validate()?;
        if !request.provenance.is_recursive_input_eligible() {
            let reason =
                "source provenance is ineligible for autonomous issue mutation".to_string();
            self.store
                .record_rejection(cycle_id, request, reason.clone())?;
            self.store.fail_cycle(cycle_id, reason)?;
            return Err(SimardError::StewardshipProvenanceBlocked {
                identity: request.identity.as_str().to_string(),
            });
        }

        match self.store.reserve(cycle_id, request)? {
            ReservationDecision::Completed(issue) => {
                return Ok(IssueMutationOutcome::AlreadyCompleted { issue });
            }
            ReservationDecision::Unfinished => {
                self.store.fail_cycle(
                    cycle_id,
                    format!(
                        "unfinished reservation '{}' requires operator reconciliation",
                        request.identity.as_str()
                    ),
                )?;
                return Err(SimardError::StewardshipUnfinishedReservation {
                    identity: request.identity.as_str().to_string(),
                });
            }
            ReservationDecision::Reserved => {}
        }

        let result = match &request.mutation {
            IssueMutation::Create {
                title,
                body,
                labels,
                assignees,
            } => transport.create_issue(
                &request.repo,
                &request.identity,
                title,
                body,
                labels,
                assignees,
            ),
            IssueMutation::Edit {
                number,
                title,
                body,
            } => transport.edit_issue(&request.repo, *number, title.as_deref(), body.as_deref()),
            IssueMutation::Close { number } => transport.close_issue(&request.repo, *number),
            IssueMutation::Reopen { number } => transport.reopen_issue(&request.repo, *number),
        };

        match result {
            Ok(issue) => {
                self.store.complete(&request.identity, issue.clone())?;
                Ok(IssueMutationOutcome::Completed { issue })
            }
            Err(error) => {
                self.store
                    .mark_ambiguous(cycle_id, &request.identity, error.to_string())?;
                Err(error)
            }
        }
    }

    pub fn execute_github(
        &mut self,
        cycle_id: &CycleId,
        request: &GitHubMutationRequest,
        mutation: impl FnOnce() -> SimardResult<GitHubMutationResult>,
    ) -> SimardResult<GitHubMutationOutcome> {
        request.validate()?;
        if !request.provenance.is_recursive_input_eligible() {
            let reason =
                "source provenance is ineligible for autonomous GitHub mutation".to_string();
            self.store
                .record_github_rejection(cycle_id, request, reason.clone())?;
            self.store.fail_cycle(cycle_id, reason)?;
            return Err(SimardError::StewardshipProvenanceBlocked {
                identity: request.identity.as_str().to_string(),
            });
        }

        match self.store.reserve_github(cycle_id, request)? {
            GitHubReservationDecision::Completed(result) => {
                return Ok(GitHubMutationOutcome::AlreadyCompleted { result });
            }
            GitHubReservationDecision::Unfinished => {
                let reason = format!(
                    "unfinished GitHub mutation reservation '{}' requires operator reconciliation",
                    request.identity.as_str()
                );
                self.store.fail_cycle(cycle_id, reason)?;
                return Err(SimardError::StewardshipUnfinishedReservation {
                    identity: request.identity.as_str().to_string(),
                });
            }
            GitHubReservationDecision::Reserved => {}
        }

        match mutation() {
            Ok(result) => {
                self.store
                    .complete_github(&request.identity, result.clone())?;
                Ok(GitHubMutationOutcome::Completed { result })
            }
            Err(error) => {
                self.store
                    .mark_github_ambiguous(cycle_id, &request.identity, error.to_string())?;
                Err(error)
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create_issue(
    cycle_id: CycleId,
    identity: IssueMutationIdentity,
    provenance: ArtifactProvenance,
    repo: &str,
    title: &str,
    body: &str,
    labels: Vec<String>,
    assignees: Vec<String>,
) -> SimardResult<crate::stewardship::GhIssue> {
    create_issue_inner(
        cycle_id, identity, provenance, repo, title, body, labels, assignees,
    )
}

#[allow(clippy::too_many_arguments)]
fn create_issue_inner(
    cycle_id: CycleId,
    identity: IssueMutationIdentity,
    provenance: ArtifactProvenance,
    repo: &str,
    title: &str,
    body: &str,
    labels: Vec<String>,
    assignees: Vec<String>,
) -> SimardResult<crate::stewardship::GhIssue> {
    let request = IssueMutationRequest::create_with_metadata(
        repo, identity, provenance, title, body, labels, assignees,
    )?;
    let mut guard = MutationGuard::from_default_store();
    guard.begin_cycle(cycle_id.clone(), IssueMutationLimit::configured()?)?;
    let transport = crate::stewardship::RealGhClient::new();
    match guard.execute(&cycle_id, &request, &transport)? {
        IssueMutationOutcome::Completed { issue }
        | IssueMutationOutcome::AlreadyCompleted { issue } => Ok(issue),
    }
}
