use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{SimardError, SimardResult};
use crate::persistence::persist_json;
use crate::stewardship::GhIssue;
use crate::stewardship::types::{
    ArtifactProvenance, CycleId, GitHubMutationRequest, GitHubMutationResult,
    IssueMutationIdentity, IssueMutationLimit, IssueMutationRequest, LineageId,
};

const STORE_NAME: &str = "stewardship-issue-mutations";

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CycleRecord {
    id: CycleId,
    limit: IssueMutationLimit,
    reservations: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_reason: Option<String>,
}

fn ensure_cycle_healthy(journal: &MutationJournal, cycle_id: &CycleId) -> SimardResult<()> {
    let cycle = journal.cycles.get(cycle_id.as_str()).ok_or_else(|| {
        SimardError::StewardshipInvalidMutation {
            field: "cycle_id",
            reason: "cycle has not been explicitly started".to_string(),
        }
    })?;
    if let Some(reason) = &cycle.failed_reason {
        return Err(SimardError::StewardshipMutationCycleFailed {
            cycle_id: cycle.id.as_str().to_string(),
            reason: reason.clone(),
        });
    }
    Ok(())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum MutationStatus {
    Reserved,
    Ambiguous {
        reason: String,
    },
    Completed {
        issue: GhIssue,
        #[serde(default)]
        provenance: ArtifactProvenance,
    },
    Rejected {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MutationRecord {
    cycle_id: CycleId,
    request: IssueMutationRequest,
    status: MutationStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum GitHubMutationStatus {
    Reserved,
    Ambiguous { reason: String },
    Completed { result: GitHubMutationResult },
    Rejected { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct GitHubMutationRecord {
    cycle_id: CycleId,
    request: GitHubMutationRequest,
    status: GitHubMutationStatus,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MutationJournal {
    version: u16,
    #[serde(default)]
    cycles: BTreeMap<String, CycleRecord>,
    #[serde(default)]
    mutations: BTreeMap<String, MutationRecord>,
    #[serde(default)]
    github_mutations: BTreeMap<String, GitHubMutationRecord>,
}

impl Default for MutationJournal {
    fn default() -> Self {
        Self {
            version: 1,
            cycles: BTreeMap::new(),
            mutations: BTreeMap::new(),
            github_mutations: BTreeMap::new(),
        }
    }
}

pub(crate) enum ReservationDecision {
    Reserved,
    Completed(GhIssue),
    Unfinished,
}

pub(crate) enum GitHubReservationDecision {
    Reserved,
    Completed(GitHubMutationResult),
    Unfinished,
}

pub struct MutationStore {
    path: PathBuf,
}

impl MutationStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_path() -> PathBuf {
        crate::state_root::simard_state_root()
            .join("state")
            .join("stewardship-issue-mutations.json")
    }

    pub fn initialize_empty(&self) -> SimardResult<()> {
        if self.path.exists() {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "initialize-empty".to_string(),
                path: self.path.clone(),
                reason: "journal already exists".to_string(),
            });
        }
        self.update(|_| Ok(()))
    }

    pub fn begin_cycle(&self, id: CycleId, limit: IssueMutationLimit) -> SimardResult<()> {
        if std::env::var("SIMARD_REQUIRE_EXISTING_MUTATION_JOURNAL").as_deref() == Ok("1")
            && !self.path.is_file()
        {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "require-existing".to_string(),
                path: self.path.clone(),
                reason: "trusted mutation journal is missing; operator reconciliation is required"
                    .to_string(),
            });
        }
        self.update(|journal| {
            if let Some(existing) = journal.cycles.get(id.as_str()) {
                if existing.limit != limit {
                    return Err(SimardError::StewardshipInvalidMutation {
                        field: "mutation_limit",
                        reason: "cannot change the limit of an existing cycle".to_string(),
                    });
                }
                if let Some(reason) = &existing.failed_reason {
                    return Err(SimardError::StewardshipMutationCycleFailed {
                        cycle_id: existing.id.as_str().to_string(),
                        reason: reason.clone(),
                    });
                }

                return Ok(());
            }
            journal.cycles.insert(
                id.as_str().to_string(),
                CycleRecord {
                    id,
                    limit,
                    reservations: 0,
                    failed_reason: None,
                },
            );
            Ok(())
        })
    }

    pub fn cycle_failure(&self, id: &CycleId) -> SimardResult<Option<String>> {
        self.update(|journal| {
            Ok(journal
                .cycles
                .get(id.as_str())
                .and_then(|cycle| cycle.failed_reason.clone()))
        })
    }

    pub fn stewardship_issue_numbers(&self, repo: &str) -> SimardResult<BTreeSet<u64>> {
        self.update(|journal| {
            Ok(journal
                .mutations
                .values()
                .filter(|record| record.request.repo == repo)
                .filter_map(|record| match &record.status {
                    MutationStatus::Completed { issue, .. } => Some(issue.number),
                    _ => None,
                })
                .collect())
        })
    }

    pub(crate) fn reserve(
        &self,
        cycle_id: &CycleId,
        request: &IssueMutationRequest,
    ) -> SimardResult<ReservationDecision> {
        self.update(|journal| {
            ensure_cycle_healthy(journal, cycle_id)?;
            let key = request.identity.as_str().to_string();
            if let Some(existing) = journal.mutations.get(&key) {
                if !requests_compatible(&existing.request, request) {
                    return Err(SimardError::StewardshipMutationIdentityConflict { identity: key });
                }
                return Ok(match &existing.status {
                    MutationStatus::Completed { issue, .. } => {
                        ReservationDecision::Completed(issue.clone())
                    }
                    MutationStatus::Reserved | MutationStatus::Ambiguous { .. } => {
                        ReservationDecision::Unfinished
                    }
                    MutationStatus::Rejected { reason } => {
                        return Err(SimardError::StewardshipInvalidMutation {
                            field: "request",
                            reason: reason.clone(),
                        });
                    }
                });
            }

            reserve_cycle_budget(journal, cycle_id)?;
            journal.mutations.insert(
                key,
                MutationRecord {
                    cycle_id: cycle_id.clone(),
                    request: request.clone(),
                    status: MutationStatus::Reserved,
                },
            );
            Ok(ReservationDecision::Reserved)
        })
    }

    pub(crate) fn reserve_github(
        &self,
        cycle_id: &CycleId,
        request: &GitHubMutationRequest,
    ) -> SimardResult<GitHubReservationDecision> {
        self.update(|journal| {
            ensure_cycle_healthy(journal, cycle_id)?;
            let key = request.identity.as_str().to_string();
            if let Some(existing) = journal.github_mutations.get(&key) {
                if existing.request != *request {
                    return Err(SimardError::StewardshipMutationIdentityConflict { identity: key });
                }
                return Ok(match &existing.status {
                    GitHubMutationStatus::Completed { result } => {
                        GitHubReservationDecision::Completed(result.clone())
                    }
                    GitHubMutationStatus::Reserved | GitHubMutationStatus::Ambiguous { .. } => {
                        GitHubReservationDecision::Unfinished
                    }
                    GitHubMutationStatus::Rejected { reason } => {
                        return Err(SimardError::StewardshipInvalidMutation {
                            field: "request",
                            reason: reason.clone(),
                        });
                    }
                });
            }

            reserve_cycle_budget(journal, cycle_id)?;
            journal.github_mutations.insert(
                key,
                GitHubMutationRecord {
                    cycle_id: cycle_id.clone(),
                    request: request.clone(),
                    status: GitHubMutationStatus::Reserved,
                },
            );
            Ok(GitHubReservationDecision::Reserved)
        })
    }

    pub(crate) fn complete(
        &self,
        identity: &IssueMutationIdentity,
        issue: GhIssue,
    ) -> SimardResult<()> {
        self.update(|journal| {
            let record = journal
                .mutations
                .get_mut(identity.as_str())
                .ok_or_else(|| SimardError::StewardshipInvalidMutation {
                    field: "mutation_identity",
                    reason: "completion has no durable reservation".to_string(),
                })?;
            record.status = MutationStatus::Completed {
                issue,
                provenance: ArtifactProvenance::stewardship(LineageId::new(identity.as_str())?),
            };
            Ok(())
        })
    }

    pub(crate) fn complete_github(
        &self,
        identity: &IssueMutationIdentity,
        result: GitHubMutationResult,
    ) -> SimardResult<()> {
        self.update(|journal| {
            let record = journal
                .github_mutations
                .get_mut(identity.as_str())
                .ok_or_else(|| SimardError::StewardshipInvalidMutation {
                    field: "mutation_identity",
                    reason: "completion has no durable reservation".to_string(),
                })?;
            record.status = GitHubMutationStatus::Completed { result };
            Ok(())
        })
    }

    pub(crate) fn mark_ambiguous(
        &self,
        cycle_id: &CycleId,
        identity: &IssueMutationIdentity,
        reason: String,
    ) -> SimardResult<()> {
        self.update(|journal| {
            let record = journal
                .mutations
                .get_mut(identity.as_str())
                .ok_or_else(|| SimardError::StewardshipInvalidMutation {
                    field: "mutation_identity",
                    reason: "ambiguous outcome has no durable reservation".to_string(),
                })?;
            record.status = MutationStatus::Ambiguous {
                reason: reason.clone(),
            };
            let cycle = journal.cycles.get_mut(cycle_id.as_str()).ok_or_else(|| {
                SimardError::StewardshipInvalidMutation {
                    field: "cycle_id",
                    reason: "ambiguous outcome has no durable cycle".to_string(),
                }
            })?;
            cycle.failed_reason = Some(reason);
            Ok(())
        })
    }

    pub(crate) fn mark_github_ambiguous(
        &self,
        cycle_id: &CycleId,
        identity: &IssueMutationIdentity,
        reason: String,
    ) -> SimardResult<()> {
        self.update(|journal| {
            let record = journal
                .github_mutations
                .get_mut(identity.as_str())
                .ok_or_else(|| SimardError::StewardshipInvalidMutation {
                    field: "mutation_identity",
                    reason: "ambiguous outcome has no durable reservation".to_string(),
                })?;
            record.status = GitHubMutationStatus::Ambiguous {
                reason: reason.clone(),
            };
            let cycle = journal.cycles.get_mut(cycle_id.as_str()).ok_or_else(|| {
                SimardError::StewardshipInvalidMutation {
                    field: "cycle_id",
                    reason: "ambiguous outcome has no durable cycle".to_string(),
                }
            })?;
            cycle.failed_reason = Some(reason);
            Ok(())
        })
    }

    pub(crate) fn fail_cycle(&self, cycle_id: &CycleId, reason: String) -> SimardResult<()> {
        self.update(|journal| {
            let cycle = journal.cycles.get_mut(cycle_id.as_str()).ok_or_else(|| {
                SimardError::StewardshipInvalidMutation {
                    field: "cycle_id",
                    reason: "failure has no durable cycle".to_string(),
                }
            })?;
            cycle.failed_reason = Some(reason);
            Ok(())
        })
    }

    pub(crate) fn record_rejection(
        &self,
        cycle_id: &CycleId,
        request: &IssueMutationRequest,
        reason: String,
    ) -> SimardResult<()> {
        self.update(|journal| {
            let key = request.identity.as_str().to_string();
            if let Some(existing) = journal.mutations.get(&key) {
                if !requests_compatible(&existing.request, request) {
                    return Err(SimardError::StewardshipMutationIdentityConflict { identity: key });
                }
                return Ok(());
            }
            journal.mutations.insert(
                key,
                MutationRecord {
                    cycle_id: cycle_id.clone(),
                    request: request.clone(),
                    status: MutationStatus::Rejected { reason },
                },
            );
            Ok(())
        })
    }

    pub(crate) fn record_github_rejection(
        &self,
        cycle_id: &CycleId,
        request: &GitHubMutationRequest,
        reason: String,
    ) -> SimardResult<()> {
        self.update(|journal| {
            let key = request.identity.as_str().to_string();
            if let Some(existing) = journal.github_mutations.get(&key) {
                if existing.request != *request {
                    return Err(SimardError::StewardshipMutationIdentityConflict { identity: key });
                }
                return Ok(());
            }
            journal.github_mutations.insert(
                key,
                GitHubMutationRecord {
                    cycle_id: cycle_id.clone(),
                    request: request.clone(),
                    status: GitHubMutationStatus::Rejected { reason },
                },
            );
            Ok(())
        })
    }

    fn update<T>(
        &self,
        operation: impl FnOnce(&mut MutationJournal) -> SimardResult<T>,
    ) -> SimardResult<T> {
        StoreLock::validate_store_file(&self.path)?;
        let _lock = StoreLock::acquire(&self.path)?;
        let mut journal = load_journal(&self.path)?;
        if journal.version != 1 {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "validate-schema".to_string(),
                path: self.path.clone(),
                reason: format!("unsupported journal version {}", journal.version),
            });
        }
        let result = operation(&mut journal);
        persist_json(STORE_NAME, &self.path, &journal)?;
        result
    }
}

fn reserve_cycle_budget(journal: &mut MutationJournal, cycle_id: &CycleId) -> SimardResult<()> {
    let cycle = journal.cycles.get_mut(cycle_id.as_str()).ok_or_else(|| {
        SimardError::StewardshipInvalidMutation {
            field: "cycle_id",
            reason: "cycle has not been explicitly started".to_string(),
        }
    })?;
    if let Some(reason) = &cycle.failed_reason {
        return Err(SimardError::StewardshipMutationCycleFailed {
            cycle_id: cycle.id.as_str().to_string(),
            reason: reason.clone(),
        });
    }
    if cycle.reservations >= cycle.limit.get() {
        cycle.failed_reason = Some(format!(
            "GitHub mutation limit {} exceeded",
            cycle.limit.get()
        ));
        return Err(SimardError::StewardshipMutationBudgetExceeded {
            cycle_id: cycle.id.as_str().to_string(),
            limit: cycle.limit.get(),
        });
    }
    cycle.reservations += 1;
    Ok(())
}

fn requests_compatible(existing: &IssueMutationRequest, incoming: &IssueMutationRequest) -> bool {
    if existing.repo != incoming.repo
        || existing.identity != incoming.identity
        || existing.provenance != incoming.provenance
    {
        return false;
    }
    match (&existing.mutation, &incoming.mutation) {
        (
            crate::stewardship::types::IssueMutation::Create {
                labels: left_labels,
                assignees: left_assignees,
                ..
            },
            crate::stewardship::types::IssueMutation::Create {
                labels: right_labels,
                assignees: right_assignees,
                ..
            },
        ) => left_labels == right_labels && left_assignees == right_assignees,
        (
            crate::stewardship::types::IssueMutation::Edit {
                number: left_number,
                title: left_title,
                body: left_body,
            },
            crate::stewardship::types::IssueMutation::Edit {
                number: right_number,
                title: right_title,
                body: right_body,
            },
        ) => left_number == right_number && left_title == right_title && left_body == right_body,
        (
            crate::stewardship::types::IssueMutation::Close { number: left },
            crate::stewardship::types::IssueMutation::Close { number: right },
        )
        | (
            crate::stewardship::types::IssueMutation::Reopen { number: left },
            crate::stewardship::types::IssueMutation::Reopen { number: right },
        ) => left == right,
        _ => false,
    }
}

fn load_journal(path: &Path) -> SimardResult<MutationJournal> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MutationJournal::default());
        }
        Err(error) => {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "open".to_string(),
                path: path.to_path_buf(),
                reason: error.to_string(),
            });
        }
    };
    StoreLock::validate_open_file(&file, path)?;
    let mut payload = Vec::new();
    file.read_to_end(&mut payload)
        .map_err(|error| SimardError::PersistentStoreIo {
            store: STORE_NAME.to_string(),
            action: "read".to_string(),
            path: path.to_path_buf(),
            reason: error.to_string(),
        })?;
    serde_json::from_slice(&payload).map_err(|error| SimardError::PersistentStoreIo {
        store: STORE_NAME.to_string(),
        action: "deserialize".to_string(),
        path: path.to_path_buf(),
        reason: error.to_string(),
    })
}

struct StoreLock {
    file: std::fs::File,
}

impl StoreLock {
    fn acquire(store_path: &Path) -> SimardResult<Self> {
        let lock_path = store_path.with_extension("lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "create-lock-dir".to_string(),
                path: parent.to_path_buf(),
                reason: error.to_string(),
            })?;
        }
        let mut options = OpenOptions::new();
        options.create(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(&lock_path)
            .map_err(|error| SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "open-lock".to_string(),
                path: lock_path.clone(),
                reason: error.to_string(),
            })?;
        Self::validate_open_file(&file, &lock_path)?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(SimardError::PersistentStoreIo {
                    store: STORE_NAME.to_string(),
                    action: "lock".to_string(),
                    path: lock_path,
                    reason: std::io::Error::last_os_error().to_string(),
                });
            }
        }
        Ok(Self { file })
    }

    fn validate_store_file(path: &Path) -> SimardResult<()> {
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => {
                return Err(SimardError::PersistentStoreIo {
                    store: STORE_NAME.to_string(),
                    action: "inspect".to_string(),
                    path: path.to_path_buf(),
                    reason: error.to_string(),
                });
            }
        };
        if !metadata.file_type().is_file() {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "validate-file-type".to_string(),
                path: path.to_path_buf(),
                reason: "mutation journal must be a regular file, not a symlink".to_string(),
            });
        }
        #[cfg(unix)]
        Self::validate_unix_owner_mode(&metadata, path)?;
        Ok(())
    }

    fn validate_open_file(file: &std::fs::File, path: &Path) -> SimardResult<()> {
        let metadata = file
            .metadata()
            .map_err(|error| SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "inspect-open-file".to_string(),
                path: path.to_path_buf(),
                reason: error.to_string(),
            })?;
        #[cfg(unix)]
        Self::validate_unix_owner_mode(&metadata, path)?;
        Ok(())
    }

    #[cfg(unix)]
    fn validate_unix_owner_mode(metadata: &std::fs::Metadata, path: &Path) -> SimardResult<()> {
        use std::os::unix::fs::MetadataExt;

        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "validate-owner".to_string(),
                path: path.to_path_buf(),
                reason: "file is not owned by the current user".to_string(),
            });
        }
        if metadata.mode() & 0o077 != 0 {
            return Err(SimardError::PersistentStoreIo {
                store: STORE_NAME.to_string(),
                action: "validate-mode".to_string(),
                path: path.to_path_buf(),
                reason: "file permissions must not grant group or other access".to_string(),
            });
        }
        Ok(())
    }
}

impl Drop for StoreLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            unsafe {
                libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}
