use std::fmt;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use super::super::atomic_file::{save_deterministic_json, SaveError};
use super::super::json::to_deterministic_json_bytes;
use super::super::migration::strict_schema_version;
use super::super::migration_batch::{ExpectedOriginal, MigratedEntry};
use super::super::project_file::directory::ProjectDirectory;
use super::super::project_file::open_existing_project_file;
use super::super::repository::{
    ArtifactCommitError, ArtifactCommitOutcome, ArtifactRepository, ArtifactWriteCategory,
    ArtifactWriteDiagnostic, ArtifactWriteError, ArtifactWriteStage, CanonicalArtifactWrite,
    CanonicalWritePlan,
};
use super::super::schema::{SchemaHeader, SchemaVersion};
#[cfg(test)]
use super::super::storage_estimate::TRANSACTION_METADATA_SAFETY_BYTES;
use super::super::storage_estimate::{
    admit_transaction_storage, StorageAdmission, StorageAdmissionError, TransactionStorageInput,
};
use super::super::utc_time::now_utc_milliseconds;
use super::{
    backup_artifact_path, is_lowercase_sha256, staged_artifact_path, LockedProject,
    ProjectRelativePath, TransactionId, TransactionManifest, TransactionModelError,
    TransactionOperation, TransactionState, TransactionStateRecord, WritePermit, WritePermitError,
    TRANSACTION_SCHEMA_VERSION,
};

const MAX_TRANSACTION_ID_ATTEMPTS: usize = 128;

#[cfg(test)]
macro_rules! check_prepare_hook {
    ($hooks:ident, $point:expr, $operation:expr) => {
        $hooks.check($point, $operation)
    };
}

/// 서로 다른 Rust 타입의 JSON을 직렬화된 소유 바이트로 모으는 내부 plan이다.
#[derive(Default)]
pub(crate) struct TransactionPlan {
    changes: Vec<PlannedChange>,
    minimum_required_bytes: u64,
}

struct PlannedChange {
    target: ProjectRelativePath,
    payload: PlannedPayload,
}

// migration의 엄격한 schema 증가와 일반 artifact의 동일 schema 교체를 혼동하지 않는다.
enum PlannedPayload {
    Ordinary(Vec<u8>),
    Migration {
        bytes: Vec<u8>,
        expected: ExpectedOriginal,
    },
    Canonical {
        write: CanonicalArtifactWrite,
        directory: Rc<ProjectDirectory>,
    },
}
impl PlannedPayload {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Ordinary(bytes) | Self::Migration { bytes, .. } => bytes,
            Self::Canonical { write, .. } => write.bytes(),
        }
    }
    fn expected_migration(&self) -> Option<&ExpectedOriginal> {
        match self {
            Self::Migration { expected, .. } => Some(expected),
            _ => None,
        }
    }
}

impl TransactionPlan {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// 값을 즉시 결정적 JSON으로 만들기 때문에 plan은 호출자 값의 lifetime을 빌리지 않는다.
    pub(crate) fn add_json<T>(
        &mut self,
        target: &str,
        value: &T,
    ) -> Result<(), TransactionPrepareError>
    where
        T: Serialize + ?Sized,
    {
        let target = ProjectRelativePath::parse(target)
            .map_err(TransactionModelError::from)
            .map_err(TransactionPrepareError::Model)?;
        if self.changes.iter().any(|change| change.target == target) {
            return Err(TransactionPrepareError::Model(
                TransactionModelError::DuplicateTarget { target },
            ));
        }
        let bytes = to_deterministic_json_bytes(value).map_err(|source| {
            TransactionPrepareError::Serialization {
                target: target.clone(),
                source,
            }
        })?;
        validate_managed_json(&bytes).map_err(|source| {
            TransactionPrepareError::InvalidManagedJson {
                target: target.clone(),
                artifact: ArtifactKind::Staged,
                source,
            }
        })?;
        self.changes.push(PlannedChange {
            target,
            payload: PlannedPayload::Ordinary(bytes),
        });
        Ok(())
    }

    /// M1-8A가 만든 검증 결과만 migration staged 입력으로 받을 수 있다.
    pub(crate) fn add_migrated_entry(
        &mut self,
        entry: MigratedEntry,
    ) -> Result<(SchemaVersion, SchemaVersion), TransactionPrepareError> {
        let (target, expected_original, source_version, target_version, bytes) =
            entry.into_transaction_parts();
        if self.changes.iter().any(|change| change.target == target) {
            return Err(TransactionPrepareError::Model(
                TransactionModelError::DuplicateTarget { target },
            ));
        }
        self.changes.push(PlannedChange {
            target,
            payload: PlannedPayload::Migration {
                bytes,
                expected: expected_original,
            },
        });
        Ok((source_version, target_version))
    }

    pub(crate) fn estimated_staged_bytes(&self) -> Result<u64, TransactionPrepareError> {
        checked_sum(
            self.changes
                .iter()
                .map(|change| change.payload.bytes().len() as u64),
        )
    }

    /// migration preflight처럼 공통 계산보다 큰 하한이 있으면 admission이
    /// 두 추정치 중 작은 값을 선택하지 못하게 한다.
    pub(crate) fn require_minimum_storage(&mut self, required_bytes: u64) {
        self.minimum_required_bytes = self.minimum_required_bytes.max(required_bytes);
    }

    pub(crate) fn prepare<'project, 'lock, 'guard>(
        self,
        project: &'project LockedProject<'lock>,
        permit: WritePermit<'guard>,
    ) -> Result<PreparedTransaction<'project, 'lock, 'guard>, TransactionPrepareError> {
        prepare_internal(
            self,
            project,
            permit,
            #[cfg(test)]
            &NoPrepareHooks,
        )
    }

    #[cfg(test)]
    pub(super) fn prepare_for_test<'project, 'lock>(
        self,
        project: &'project LockedProject<'lock>,
    ) -> Result<PreparedTransaction<'project, 'lock, 'static>, TransactionPrepareError> {
        if self.changes.is_empty() {
            return Err(TransactionPrepareError::EmptyPlan);
        }
        let targets = self
            .changes
            .iter()
            .map(|change| change.target.clone())
            .collect();
        let permit = super::test_write_permit(project.fingerprint(), targets);
        self.prepare(project, permit)
    }
}

/// Prepared 상태와 ProjectLock 수명을 묶는다. Drop은 복구 자료를 삭제하지 않는다.
/// G4 경로는 별도의 wrapper가 repository와 namespace guard도 함께 빌린다.
pub(crate) struct PreparedTransaction<'project, 'lock, 'guard> {
    pub(super) project: &'project LockedProject<'lock>,
    pub(super) permit: WritePermit<'guard>,
    pub(super) transaction_id: TransactionId,
    pub(super) transaction_directory: PathBuf,
    pub(super) manifest: TransactionManifest,
    estimated_required_bytes: u64,
    storage_admission: StorageAdmission,
}

/// 내부 prepared를 꺼내는 API를 제공하지 않는다. drop은 M1의 복구 자료 보존 계약을 따른다.
pub(crate) struct PreparedArtifactTransaction<'repo, 'access, 'runtime, 'guard> {
    transaction: PreparedTransaction<'repo, 'repo, 'guard>,
    repository: &'repo ArtifactRepository<'access, 'runtime>,
    directories: Vec<Rc<ProjectDirectory>>,
}
impl PreparedArtifactTransaction<'_, '_, '_, '_> {
    pub(crate) fn transaction_id(&self) -> &TransactionId {
        self.transaction.transaction_id()
    }
    pub(crate) fn storage_admission(&self) -> StorageAdmission {
        self.transaction.storage_admission()
    }
    pub(crate) fn commit(self) -> Result<ArtifactCommitOutcome, ArtifactCommitError> {
        let Self {
            transaction,
            repository,
            directories,
        } = self;
        // 디렉터리는 M1 apply/rollback/cleanup 전체에서 유지하고 개별 target read handle은 없다.
        #[cfg(not(test))]
        let result = transaction
            .commit()
            .map(ArtifactCommitOutcome::new)
            .map_err(ArtifactCommitError::new);
        #[cfg(test)]
        let result = super::test_support::commit_with_configured_test_hooks(transaction)
            .map(ArtifactCommitOutcome::new)
            .map_err(ArtifactCommitError::new);
        drop(directories);
        let _ = repository;
        result
    }
    #[cfg(test)]
    pub(crate) fn inner_for_test(&self) -> &PreparedTransaction<'_, '_, '_> {
        &self.transaction
    }
}
impl fmt::Debug for PreparedArtifactTransaction<'_, '_, '_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PreparedArtifactTransaction([redacted prepared state])")
    }
}

pub(crate) fn prepare_canonical<'repo, 'access, 'runtime, 'guard>(
    plan: CanonicalWritePlan,
    repository: &'repo ArtifactRepository<'access, 'runtime>,
    permit: WritePermit<'guard>,
) -> Result<PreparedArtifactTransaction<'repo, 'access, 'runtime, 'guard>, ArtifactWriteError> {
    #[cfg(test)]
    super::test_support::ConfiguredPrepareHooks::entered();
    prepare_canonical_internal(
        plan,
        repository,
        permit,
        #[cfg(test)]
        &super::test_support::ConfiguredPrepareHooks,
    )
}

fn prepare_canonical_internal<'repo, 'access, 'runtime, 'guard>(
    plan: CanonicalWritePlan,
    repository: &'repo ArtifactRepository<'access, 'runtime>,
    permit: WritePermit<'guard>,
    #[cfg(test)] hooks: &impl PrepareHooks,
) -> Result<PreparedArtifactTransaction<'repo, 'access, 'runtime, 'guard>, ArtifactWriteError> {
    let mut transaction_plan = TransactionPlan::new();
    let mut directories = Vec::new();
    for write in plan.into_writes() {
        let directory = Rc::new(write.guard(repository)?);
        transaction_plan.changes.push(PlannedChange {
            target: write.target().clone(),
            payload: PlannedPayload::Canonical {
                write,
                directory: Rc::clone(&directory),
            },
        });
        directories.push(directory);
    }
    let transaction = prepare_internal(
        transaction_plan,
        repository.write_project(),
        permit,
        #[cfg(test)]
        hooks,
    )
    .map_err(ArtifactWriteError::prepare)?;
    Ok(PreparedArtifactTransaction {
        transaction,
        repository,
        directories,
    })
}

#[cfg(test)]
pub(crate) fn prepare_canonical_with_hooks<'repo, 'access, 'runtime, 'guard>(
    plan: CanonicalWritePlan,
    repository: &'repo ArtifactRepository<'access, 'runtime>,
    permit: WritePermit<'guard>,
    hooks: &impl PrepareHooks,
) -> Result<PreparedArtifactTransaction<'repo, 'access, 'runtime, 'guard>, ArtifactWriteError> {
    prepare_canonical_internal(plan, repository, permit, hooks)
}

impl PreparedTransaction<'_, '_, '_> {
    pub(crate) fn transaction_id(&self) -> &TransactionId {
        &self.transaction_id
    }

    pub(crate) fn project_fingerprint(&self) -> &str {
        self.project.fingerprint()
    }

    pub(crate) fn transaction_directory(&self) -> &Path {
        &self.transaction_directory
    }

    pub(crate) fn manifest(&self) -> &TransactionManifest {
        &self.manifest
    }

    pub(crate) fn estimated_required_bytes(&self) -> u64 {
        self.estimated_required_bytes
    }

    pub(crate) fn storage_admission(&self) -> StorageAdmission {
        self.storage_admission
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrepareStage {
    ValidateTarget,
    ValidateSchemaTransition,
    CreateSystemDirectory,
    CreateTransactionDirectory,
    CreateArtifactDirectories,
    WritePreparingState,
    ReadOriginal,
    WriteBackup,
    WriteStaged,
    WriteManifest,
    VerifyManifest,
    WritePreparedState,
    SyncDirectory,
}

impl fmt::Display for PrepareStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::ValidateTarget => "validate transaction target",
            Self::ValidateSchemaTransition => "validate transaction schema transition",
            Self::CreateSystemDirectory => "create transaction system directory",
            Self::CreateTransactionDirectory => "allocate transaction directory",
            Self::CreateArtifactDirectories => "create transaction artifact directories",
            Self::WritePreparingState => "write Preparing state",
            Self::ReadOriginal => "read original target",
            Self::WriteBackup => "write backup artifact",
            Self::WriteStaged => "write staged artifact",
            Self::WriteManifest => "write immutable manifest",
            Self::VerifyManifest => "verify immutable manifest",
            Self::WritePreparedState => "write Prepared state",
            Self::SyncDirectory => "sync transaction directory",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactKind {
    Staged,
}

#[derive(Debug)]
pub(crate) enum ManagedJsonError {
    Parse(serde_json::Error),
    NotObject,
    InvalidSchema(serde_json::Error),
}

impl fmt::Display for ManagedJsonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(_) => formatter.write_str("artifact is not valid JSON"),
            Self::NotObject => formatter.write_str("managed JSON must be a top-level object"),
            Self::InvalidSchema(_) => {
                formatter.write_str("managed JSON requires a positive schemaVersion")
            }
        }
    }
}

impl std::error::Error for ManagedJsonError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Parse(source) | Self::InvalidSchema(source) => Some(source),
            Self::NotObject => None,
        }
    }
}

#[derive(Debug)]
pub(crate) enum PrepareFailureSource {
    Io(io::Error),
    ExpectedOriginalMismatch(io::Error),
    SchemaTransitionMismatch(io::Error),
    AtomicSave(SaveError),
    Json(ManagedJsonError),
    Model(TransactionModelError),
    Artifact(ArtifactWriteError),
}

impl std::error::Error for PrepareFailureSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(source)
            | Self::ExpectedOriginalMismatch(source)
            | Self::SchemaTransitionMismatch(source) => Some(source),
            Self::AtomicSave(source) => Some(source),
            Self::Json(source) => Some(source),
            Self::Model(source) => Some(source),
            Self::Artifact(source) => Some(source),
        }
    }
}

impl fmt::Display for PrepareFailureSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("filesystem operation failed"),
            Self::ExpectedOriginalMismatch(_) => {
                formatter.write_str("migration source differs from expected original")
            }
            Self::SchemaTransitionMismatch(_) => {
                formatter.write_str("ordinary transaction schemaVersion must remain unchanged")
            }
            Self::AtomicSave(_) => formatter.write_str("atomic JSON save failed"),
            Self::Json(source) => source.fmt(formatter),
            Self::Model(source) => source.fmt(formatter),
            Self::Artifact(source) => source.fmt(formatter),
        }
    }
}

#[derive(Debug)]
pub(crate) enum TransactionPrepareError {
    EmptyPlan,
    Model(TransactionModelError),
    Serialization {
        target: ProjectRelativePath,
        source: serde_json::Error,
    },
    InvalidManagedJson {
        target: ProjectRelativePath,
        artifact: ArtifactKind,
        source: ManagedJsonError,
    },
    EstimateOverflow,
    StorageAdmission(StorageAdmissionError),
    Permit(WritePermitError),
    Failed(Box<PrepareFailure>),
    CleanupFailed {
        transaction_id: TransactionId,
        project_fingerprint: String,
        source: Box<TransactionPrepareError>,
        cleanup_error: io::Error,
    },
}

#[derive(Debug)]
pub(crate) struct PrepareFailure {
    stage: PrepareStage,
    transaction_id: Option<TransactionId>,
    project_fingerprint: String,
    target: Option<ProjectRelativePath>,
    source: PrepareFailureSource,
    cleanup_error: Option<io::Error>,
}

impl TransactionPrepareError {
    /// G4에는 경로/원문 source chain 대신 안전한 관찰만 전달한다. 원본 오류 소유권은 유지된다.
    pub(crate) fn artifact_diagnostic(&self) -> ArtifactWriteDiagnostic {
        use ArtifactWriteCategory as C;
        let mut diagnostic =
            ArtifactWriteDiagnostic::new(C::PrepareFailed, ArtifactWriteStage::Prepare, None);
        match self {
            Self::EmptyPlan => diagnostic.category = C::EmptyPlan,
            Self::EstimateOverflow => diagnostic.category = C::EstimateOverflow,
            Self::Permit(_) => diagnostic.category = C::PermitRejected,
            Self::StorageAdmission(error) => {
                diagnostic.category = C::StorageRejected;
                diagnostic.storage_query = error.kind();
                diagnostic.storage = error.admission();
                if let StorageAdmissionError::Query {
                    source: Some(source),
                    ..
                } = error
                {
                    diagnostic.io = Some(source.into());
                }
            }
            Self::Failed(failure) => {
                match &failure.source {
                    PrepareFailureSource::Artifact(error) => {
                        diagnostic = error.diagnostic().clone()
                    }
                    PrepareFailureSource::Io(error)
                    | PrepareFailureSource::ExpectedOriginalMismatch(error) => {
                        diagnostic.io = Some(error.into())
                    }
                    PrepareFailureSource::SchemaTransitionMismatch(error) => {
                        diagnostic.category = C::SchemaMismatch;
                        diagnostic.io = Some(error.into());
                    }
                    PrepareFailureSource::AtomicSave(SaveError::AtomicWrite(error)) => {
                        diagnostic.io = Some((&error.source).into());
                    }
                    PrepareFailureSource::AtomicSave(SaveError::Serialize(_))
                    | PrepareFailureSource::Json(_)
                    | PrepareFailureSource::Model(_) => {}
                }
                diagnostic.prepare_stage = Some(failure.stage);
                diagnostic.transaction_id = failure.transaction_id.clone();
                diagnostic.cleanup = failure.cleanup_error.as_ref().map(Into::into);
            }
            Self::CleanupFailed {
                source,
                transaction_id,
                cleanup_error,
                ..
            } => {
                diagnostic = source.artifact_diagnostic();
                diagnostic.transaction_id = Some(transaction_id.clone());
                diagnostic.cleanup = Some(cleanup_error.into());
            }
            Self::Model(_) | Self::Serialization { .. } | Self::InvalidManagedJson { .. } => {}
        }
        diagnostic.failures = self.artifact_failures();
        diagnostic.recovery_required = self.cleanup_error().is_some();
        diagnostic
    }

    fn artifact_failures(&self) -> Vec<super::artifact_diagnostics::FailureDiagnostic> {
        use super::artifact_diagnostics::{
            FailureDiagnostic as D, FailureRole as R, FailureStage as S,
        };
        use crate::data::repository::ArtifactSourceId;
        let mut detail = D::new(R::Primary, S::Admission, "Prepare");
        match self {
            Self::EmptyPlan => detail.category = "EmptyPlan",
            Self::EstimateOverflow => detail.category = "EstimateOverflow",
            Self::Model(source) => detail = detail.model(source),
            Self::Serialization { target, source } => {
                detail.category = "Serialization";
                detail.target = ArtifactSourceId::from_target(target);
                detail.json = Some(source.into());
            }
            Self::InvalidManagedJson {
                target,
                artifact: ArtifactKind::Staged,
                source,
            } => {
                detail = detail.managed(source);
                detail.target = ArtifactSourceId::from_target(target);
            }
            Self::Permit(source) => detail = detail.permit(source),
            Self::StorageAdmission(source) => {
                detail.category = "StorageAdmission";
                if let StorageAdmissionError::Query {
                    source: Some(source),
                    ..
                } = source
                {
                    detail = detail.io(source);
                }
            }
            Self::Failed(failure) => {
                detail.stage = S::Prepare(failure.stage);
                detail = match &failure.source {
                    PrepareFailureSource::Io(source) => {
                        detail.category = "Io";
                        detail.io(source)
                    }
                    PrepareFailureSource::ExpectedOriginalMismatch(source) => {
                        detail.category = "ExpectedOriginalMismatch";
                        detail.io(source)
                    }
                    PrepareFailureSource::SchemaTransitionMismatch(source) => {
                        detail.category = "SchemaTransitionMismatch";
                        detail.io(source)
                    }
                    PrepareFailureSource::AtomicSave(source) => detail.save(source),
                    PrepareFailureSource::Json(source) => detail.managed(source),
                    PrepareFailureSource::Model(source) => detail.model(source),
                    PrepareFailureSource::Artifact(source) => {
                        detail.category = "Artifact";
                        detail.target = source.diagnostic().target;
                        detail.secondary = source.diagnostic().failures.clone();
                        detail
                    }
                };
                detail.transaction_id = failure.transaction_id.clone();
                detail.target = failure
                    .target
                    .as_ref()
                    .and_then(ArtifactSourceId::from_target)
                    .or(detail.target);
                let mut result = vec![detail];
                if let Some(cleanup) = &failure.cleanup_error {
                    let mut cleanup = D::new(R::Cleanup, S::PrepareCleanup, "Io").io(cleanup);
                    cleanup.transaction_id = failure.transaction_id.clone();
                    result.push(cleanup);
                }
                return result;
            }
            Self::CleanupFailed {
                source,
                transaction_id,
                cleanup_error,
                ..
            } => {
                let mut result = source.artifact_failures();
                let mut cleanup = D::new(R::Cleanup, S::PrepareCleanup, "Io").io(cleanup_error);
                cleanup.transaction_id = Some(transaction_id.clone());
                result.push(cleanup);
                return result;
            }
        }
        vec![detail]
    }

    #[cfg(test)]
    pub(crate) fn implementation_original_io(&self) -> Option<&io::Error> {
        match self {
            Self::Failed(failure) => match &failure.source {
                PrepareFailureSource::Io(source) => Some(source),
                _ => None,
            },
            Self::CleanupFailed { source, .. } => source.implementation_original_io(),
            _ => None,
        }
    }

    fn failed(
        stage: PrepareStage,
        project: &LockedProject<'_>,
        transaction_id: Option<&TransactionId>,
        target: Option<&ProjectRelativePath>,
        source: PrepareFailureSource,
    ) -> Self {
        Self::Failed(Box::new(PrepareFailure {
            stage,
            transaction_id: transaction_id.cloned(),
            project_fingerprint: project.fingerprint().to_owned(),
            target: target.cloned(),
            source,
            cleanup_error: None,
        }))
    }

    fn with_cleanup_error(
        mut self,
        transaction_id: &TransactionId,
        project_fingerprint: &str,
        cleanup_error: io::Error,
    ) -> Self {
        if let Self::Failed(failure) = &mut self {
            failure.cleanup_error = Some(cleanup_error);
            self
        } else {
            Self::CleanupFailed {
                transaction_id: transaction_id.clone(),
                project_fingerprint: project_fingerprint.to_owned(),
                source: Box::new(self),
                cleanup_error,
            }
        }
    }

    pub(crate) fn cleanup_error(&self) -> Option<&io::Error> {
        match self {
            Self::Failed(failure) => failure.cleanup_error.as_ref(),
            Self::CleanupFailed { cleanup_error, .. } => Some(cleanup_error),
            _ => None,
        }
    }

    pub(crate) fn expected_original_mismatch(&self) -> bool {
        match self {
            Self::Failed(failure) => matches!(
                failure.source,
                PrepareFailureSource::ExpectedOriginalMismatch(_)
            ),
            Self::CleanupFailed { source, .. } => source.expected_original_mismatch(),
            _ => false,
        }
    }

    pub(crate) fn schema_transition_mismatch(&self) -> bool {
        match self {
            Self::Failed(failure) => matches!(
                failure.source,
                PrepareFailureSource::SchemaTransitionMismatch(_)
            ),
            Self::CleanupFailed { source, .. } => source.schema_transition_mismatch(),
            _ => false,
        }
    }
}

impl fmt::Display for TransactionPrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPlan => formatter.write_str("transaction plan contains no changes"),
            Self::Model(source) => source.fmt(formatter),
            Self::Serialization { target, .. } => {
                write!(formatter, "failed to serialize transaction target '{target}'")
            }
            Self::InvalidManagedJson {
                target,
                artifact,
                source,
            } => write!(
                formatter,
                "invalid {artifact:?} JSON for transaction target '{target}': {source}"
            ),
            Self::EstimateOverflow => {
                formatter.write_str("transaction required byte estimate overflowed")
            }
            Self::StorageAdmission(source) => source.fmt(formatter),
            Self::Permit(source) => write!(formatter, "transaction write permit rejected: {source}"),
            Self::Failed(failure) => {
                let PrepareFailure {
                    stage,
                    transaction_id,
                    project_fingerprint,
                    target,
                    cleanup_error,
                    ..
                } = failure.as_ref();
                write!(formatter, "failed to {stage} for project {project_fingerprint}")?;
                if let Some(id) = transaction_id {
                    write!(formatter, ", transaction {id}")?;
                }
                if let Some(target) = target {
                    write!(formatter, ", target '{target}'")?;
                }
                if cleanup_error.is_some() {
                    formatter.write_str("; transaction cleanup also failed")?;
                }
                Ok(())
            }
            Self::CleanupFailed {
                transaction_id,
                project_fingerprint,
                ..
            } => write!(
                formatter,
                "transaction {transaction_id} for project {project_fingerprint} failed and cleanup also failed"
            ),
        }
    }
}

impl std::error::Error for TransactionPrepareError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Model(source) => Some(source),
            Self::Serialization { source, .. } => Some(source),
            Self::InvalidManagedJson { source, .. } => Some(source),
            Self::StorageAdmission(source) => Some(source),
            Self::Permit(source) => Some(source),
            Self::Failed(failure) => Some(&failure.source),
            Self::CleanupFailed { source, .. } => Some(source),
            Self::EmptyPlan | Self::EstimateOverflow => None,
        }
    }
}

struct ResolvedChange {
    target: ProjectRelativePath,
    absolute_target: PathBuf,
    payload: PlannedPayload,
    original_size_snapshot: Option<u64>,
    original_sha256_snapshot: Option<String>,
    original_schema_snapshot: Option<SchemaVersion>,
}

struct PreparedArtifacts {
    operations: Vec<TransactionOperation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(test)]
pub(crate) enum PrepareFailPoint {
    CreateTransactionDirectory,
    TransactionDirectoryAllocated,
    PreparingState,
    BeforeOriginalRead,
    BackupCreate,
    BackupWrite,
    BackupSync,
    StagedCreate,
    StagedWrite,
    StagedSync,
    ManifestCreate,
    ManifestSync,
    ManifestVerify,
    PreparedState,
    Cleanup,
}

#[cfg(test)]
pub(crate) trait PrepareHooks {
    fn check(&self, point: PrepareFailPoint, operation: Option<u32>) -> io::Result<()>;
}

#[cfg(test)]
struct NoPrepareHooks;

#[cfg(test)]
impl PrepareHooks for NoPrepareHooks {
    fn check(&self, _point: PrepareFailPoint, _operation: Option<u32>) -> io::Result<()> {
        Ok(())
    }
}

fn prepare_internal<'project, 'lock, 'guard>(
    mut plan: TransactionPlan,
    project: &'project LockedProject<'lock>,
    mut permit: WritePermit<'guard>,
    #[cfg(test)] hooks: &impl PrepareHooks,
) -> Result<PreparedTransaction<'project, 'lock, 'guard>, TransactionPrepareError> {
    if plan.changes.is_empty() {
        return Err(TransactionPrepareError::EmptyPlan);
    }
    plan.changes
        .sort_by(|left, right| left.target.cmp(&right.target));
    for pair in plan.changes.windows(2) {
        if pair[0].target == pair[1].target {
            return Err(TransactionPrepareError::Model(
                TransactionModelError::DuplicateTarget {
                    target: pair[1].target.clone(),
                },
            ));
        }
    }
    plan.estimated_staged_bytes()?;
    let minimum_required_bytes = plan.minimum_required_bytes;

    let targets: Vec<_> = plan
        .changes
        .iter()
        .map(|change| change.target.clone())
        .collect();
    permit
        .validate_for(project.fingerprint(), &targets)
        .map_err(TransactionPrepareError::Permit)?;

    // 경로와 기존 부모를 모두 확인한 뒤에만 시스템 디렉터리를 생성한다.
    let mut resolved = Vec::with_capacity(plan.changes.len());
    for change in plan.changes {
        let absolute_target = resolve_target(project, &change.target)?;
        resolved.push(ResolvedChange {
            target: change.target,
            absolute_target,
            payload: change.payload,
            original_size_snapshot: None,
            original_sha256_snapshot: None,
            original_schema_snapshot: None,
        });
    }
    precheck_schema_transitions(project, &mut resolved)?;

    let staged_sizes = resolved
        .iter()
        .map(|change| {
            u64::try_from(change.payload.bytes().len())
                .map_err(|_| TransactionPrepareError::EstimateOverflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let backup_sizes = resolved
        .iter()
        .map(|change| change.original_size_snapshot)
        .collect::<Vec<_>>();
    let target_name_bytes = resolved
        .iter()
        .map(|change| {
            u64::try_from(change.target.as_str().len())
                .map_err(|_| TransactionPrepareError::EstimateOverflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let target_paths = resolved
        .iter()
        .map(|change| change.absolute_target.clone())
        .collect::<Vec<_>>();
    let has_owned = resolved
        .iter()
        .any(|c| matches!(c.payload, PlannedPayload::Canonical { .. }));
    let admit = if has_owned {
        super::super::storage_estimate::admit_owned_transaction_storage
    } else {
        admit_transaction_storage
    };
    let admission = admit(
        &project.transactions_root(),
        &target_paths,
        TransactionStorageInput {
            staged_sizes: &staged_sizes,
            backup_sizes: &backup_sizes,
            target_name_bytes: &target_name_bytes,
            minimum_required_bytes,
        },
    )
    .map_err(TransactionPrepareError::StorageAdmission)?;

    let transactions_root = ensure_transaction_roots(project)?;
    let (transaction_id, transaction_directory, created_at_utc) = allocate_transaction_directory(
        project,
        &transactions_root,
        #[cfg(test)]
        hooks,
    )?;

    let identity = PreparationIdentity {
        transaction_id: &transaction_id,
        transaction_directory: &transaction_directory,
        created_at_utc: &created_at_utc,
    };
    let result = prepare_allocated(
        project,
        identity,
        resolved,
        permit,
        admission,
        #[cfg(test)]
        hooks,
    );
    match result {
        Ok(prepared) => Ok(prepared),
        Err(error) => {
            #[cfg(test)]
            let cleanup_result = hooks
                .check(PrepareFailPoint::Cleanup, None)
                .and_then(|()| fs::remove_dir_all(&transaction_directory))
                .and_then(|()| sync_directory(&transactions_root));
            #[cfg(not(test))]
            let cleanup_result = fs::remove_dir_all(&transaction_directory)
                .and_then(|()| sync_directory(&transactions_root));
            match cleanup_result {
                Ok(()) => Err(error),
                Err(cleanup_error) => Err(error.with_cleanup_error(
                    &transaction_id,
                    project.fingerprint(),
                    cleanup_error,
                )),
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn prepare_with_hooks<'project, 'lock, 'guard, H>(
    plan: TransactionPlan,
    project: &'project LockedProject<'lock>,
    permit: WritePermit<'guard>,
    hooks: &H,
) -> Result<PreparedTransaction<'project, 'lock, 'guard>, TransactionPrepareError>
where
    H: PrepareHooks,
{
    prepare_internal(plan, project, permit, hooks)
}

#[cfg(test)]
pub(super) fn prepare_with_hooks_for_test<'project, 'lock, H>(
    plan: TransactionPlan,
    project: &'project LockedProject<'lock>,
    hooks: &H,
) -> Result<PreparedTransaction<'project, 'lock, 'static>, TransactionPrepareError>
where
    H: PrepareHooks,
{
    let targets = plan
        .changes
        .iter()
        .map(|change| change.target.clone())
        .collect();
    let permit = super::test_write_permit(project.fingerprint(), targets);
    prepare_with_hooks(plan, project, permit, hooks)
}

#[derive(Clone, Copy)]
struct PreparationIdentity<'a> {
    transaction_id: &'a TransactionId,
    transaction_directory: &'a Path,
    created_at_utc: &'a str,
}

fn prepare_allocated<'project, 'lock, 'guard>(
    project: &'project LockedProject<'lock>,
    identity: PreparationIdentity<'_>,
    changes: Vec<ResolvedChange>,
    permit: WritePermit<'guard>,
    storage_admission: StorageAdmission,
    #[cfg(test)] hooks: &impl PrepareHooks,
) -> Result<PreparedTransaction<'project, 'lock, 'guard>, TransactionPrepareError> {
    let PreparationIdentity {
        transaction_id,
        transaction_directory,
        created_at_utc,
    } = identity;
    #[cfg(test)]
    check_prepare_hook!(hooks, PrepareFailPoint::TransactionDirectoryAllocated, None).map_err(
        |source| {
            TransactionPrepareError::failed(
                PrepareStage::CreateTransactionDirectory,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        },
    )?;
    let has_owned = changes
        .iter()
        .any(|c| matches!(c.payload, PlannedPayload::Canonical { .. }));
    let schema_version = if has_owned {
        super::model::OWNED_TRANSACTION_SCHEMA_VERSION
    } else {
        TRANSACTION_SCHEMA_VERSION
    };
    let staged_directory = transaction_directory.join("staged");
    let backups_directory = transaction_directory.join("backups");
    fs::create_dir(&staged_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::CreateArtifactDirectories,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    fs::create_dir(&backups_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::CreateArtifactDirectories,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    sync_directory(transaction_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::SyncDirectory,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;

    let mut preparing = state_record(
        schema_version,
        transaction_id,
        project.fingerprint(),
        created_at_utc,
        TransactionState::Preparing,
    );
    if has_owned {
        preparing.original_targets = Some(
            changes
                .iter()
                .map(|change| super::model::OriginalTarget {
                    target_path: change.target.clone(),
                    original_size: change.original_size_snapshot,
                    original_sha256: change.original_sha256_snapshot.clone(),
                    original_schema_version: change.original_schema_snapshot,
                })
                .collect(),
        );
        // 모든 미래 progress와 가장 긴 enum 이름까지 첫 durable 기록 전에 검사한다.
        let mut largest = preparing.clone();
        largest.state = TransactionState::CleaningRolledBack;
        largest.applied_operations = (0..u32::try_from(changes.len())
            .map_err(|_| TransactionPrepareError::EstimateOverflow)?)
            .collect();
        super::protocol::check_state_bound(&largest).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::WritePreparingState,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        })?;
    }
    #[cfg(test)]
    check_prepare_hook!(hooks, PrepareFailPoint::PreparingState, None).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::WritePreparingState,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    if has_owned {
        super::owned_temp::checkpoint("protocol-empty", 0).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::WritePreparingState,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        })?;
    }
    save_deterministic_json(&transaction_directory.join("state.json"), &preparing).map_err(
        |source| {
            TransactionPrepareError::failed(
                PrepareStage::WritePreparingState,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::AtomicSave(source),
            )
        },
    )?;

    if has_owned {
        super::owned_temp::checkpoint("protocol-preparing", 0).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::WritePreparingState,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        })?;
    }
    let artifacts = write_artifacts(
        project,
        transaction_id,
        &staged_directory,
        &backups_directory,
        changes,
        #[cfg(test)]
        hooks,
    )?;
    let manifest = TransactionManifest {
        schema_version,
        transaction_id: transaction_id.clone(),
        created_at_utc: created_at_utc.to_owned(),
        project_fingerprint: project.fingerprint().to_owned(),
        operations: artifacts.operations,
    };
    manifest.validate().map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::WriteManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Model(source),
        )
    })?;
    if manifest.original_targets() != preparing.original_targets {
        return Err(TransactionPrepareError::failed(
            PrepareStage::WriteManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(super::protocol::invalid(
                "preparing evidence differs from verified originals",
            )),
        ));
    }
    let manifest_bytes = to_deterministic_json_bytes(&manifest).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::WriteManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(io::Error::new(io::ErrorKind::InvalidData, source)),
        )
    })?;

    if has_owned && manifest_bytes.len() as u64 > super::protocol::MAIN_RECORD_LIMIT {
        return Err(TransactionPrepareError::failed(
            PrepareStage::WriteManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(super::protocol::invalid(
                "canonical manifest exceeds metadata bound",
            )),
        ));
    }
    let manifest_path = transaction_directory.join("manifest.json");
    write_new_synced(
        &manifest_path,
        &manifest_bytes,
        #[cfg(test)]
        hooks,
        #[cfg(test)]
        PrepareFailPoint::ManifestCreate,
        #[cfg(test)]
        PrepareFailPoint::ManifestCreate,
        #[cfg(test)]
        PrepareFailPoint::ManifestSync,
        #[cfg(test)]
        None,
    )
    .map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::WriteManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    verify_manifest(
        project,
        transaction_id,
        &manifest_path,
        &manifest_bytes,
        &manifest,
        transaction_directory,
        #[cfg(test)]
        hooks,
    )?;

    if has_owned {
        super::owned_temp::checkpoint("protocol-manifest", 0).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::WriteManifest,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        })?;
    }
    if has_owned {
        super::owned_temp::initialize(transaction_directory).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::CreateArtifactDirectories,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        })?;
    }
    if has_owned {
        super::owned_temp::checkpoint("protocol-owned-directory", 0).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::CreateArtifactDirectories,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::Io(source),
            )
        })?;
    }
    let prepared_time = now_utc_milliseconds().map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::WritePreparedState,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Model(TransactionModelError::TransactionTimeFailed { source }),
        )
    })?;
    let mut prepared = state_record(
        schema_version,
        transaction_id,
        project.fingerprint(),
        &prepared_time,
        TransactionState::Prepared,
    );
    prepared.original_targets = preparing.original_targets;
    #[cfg(test)]
    check_prepare_hook!(hooks, PrepareFailPoint::PreparedState, None).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::WritePreparedState,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    save_deterministic_json(&transaction_directory.join("state.json"), &prepared).map_err(
        |source| {
            TransactionPrepareError::failed(
                PrepareStage::WritePreparedState,
                project,
                Some(transaction_id),
                None,
                PrepareFailureSource::AtomicSave(source),
            )
        },
    )?;
    sync_directory(transaction_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::SyncDirectory,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;

    Ok(PreparedTransaction {
        project,
        permit,
        transaction_id: transaction_id.clone(),
        transaction_directory: transaction_directory.to_path_buf(),
        manifest,
        estimated_required_bytes: storage_admission.required_peak_bytes(),
        storage_admission,
    })
}

fn state_record(
    schema_version: SchemaVersion,
    transaction_id: &TransactionId,
    fingerprint: &str,
    timestamp: &str,
    state: TransactionState,
) -> TransactionStateRecord {
    TransactionStateRecord {
        schema_version,
        transaction_id: transaction_id.clone(),
        updated_at_utc: timestamp.to_owned(),
        project_fingerprint: fingerprint.to_owned(),
        state,
        applied_operations: Vec::new(),
        original_targets: None,
    }
}

fn resolve_target(
    project: &LockedProject<'_>,
    target: &ProjectRelativePath,
) -> Result<PathBuf, TransactionPrepareError> {
    let absolute_target = project.canonical_root().join(target.as_str());
    let parent = absolute_target.parent().ok_or_else(|| {
        TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "target has no parent directory",
            )),
        )
    })?;
    let parent_metadata = fs::metadata(parent).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(source),
        )
    })?;
    if !parent_metadata.is_dir() {
        return Err(TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::NotADirectory,
                "target parent is not a directory",
            )),
        ));
    }
    let canonical_parent = fs::canonicalize(parent).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(source),
        )
    })?;
    if !canonical_parent.starts_with(project.canonical_root()) {
        return Err(TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "target parent resolves outside the project",
            )),
        ));
    }
    match fs::symlink_metadata(&absolute_target) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "symbolic link and junction targets are not supported in M1-5B",
            )),
        )),
        Ok(metadata) if !metadata.is_file() => Err(TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::InvalidInput,
                "existing target is not a regular file",
            )),
        )),
        Ok(_) => Ok(absolute_target),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(absolute_target),
        Err(source) => Err(TransactionPrepareError::failed(
            PrepareStage::ValidateTarget,
            project,
            None,
            Some(target),
            PrepareFailureSource::Io(source),
        )),
    }
}

fn ensure_transaction_roots(
    project: &LockedProject<'_>,
) -> Result<PathBuf, TransactionPrepareError> {
    let worldbuild = project.canonical_root().join(".worldbuild");
    ensure_private_directory(project, project.canonical_root(), &worldbuild)?;
    let transactions = worldbuild.join("transactions");
    ensure_private_directory(project, &worldbuild, &transactions)?;
    Ok(transactions)
}

fn ensure_private_directory(
    project: &LockedProject<'_>,
    parent: &Path,
    directory: &Path,
) -> Result<(), TransactionPrepareError> {
    match fs::symlink_metadata(directory) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err(TransactionPrepareError::failed(
                PrepareStage::CreateSystemDirectory,
                project,
                None,
                None,
                PrepareFailureSource::Io(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "transaction system path is not a real directory",
                )),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir(directory).map_err(|source| {
                TransactionPrepareError::failed(
                    PrepareStage::CreateSystemDirectory,
                    project,
                    None,
                    None,
                    PrepareFailureSource::Io(source),
                )
            })?;
            sync_directory(parent).map_err(|source| {
                TransactionPrepareError::failed(
                    PrepareStage::SyncDirectory,
                    project,
                    None,
                    None,
                    PrepareFailureSource::Io(source),
                )
            })?;
        }
        Err(source) => {
            return Err(TransactionPrepareError::failed(
                PrepareStage::CreateSystemDirectory,
                project,
                None,
                None,
                PrepareFailureSource::Io(source),
            ));
        }
    }
    let canonical = fs::canonicalize(directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::CreateSystemDirectory,
            project,
            None,
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    if !canonical.starts_with(project.canonical_root()) {
        return Err(TransactionPrepareError::failed(
            PrepareStage::CreateSystemDirectory,
            project,
            None,
            None,
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "transaction system directory resolves outside the project",
            )),
        ));
    }
    Ok(())
}

fn allocate_transaction_directory(
    project: &LockedProject<'_>,
    transactions_root: &Path,
    #[cfg(test)] hooks: &impl PrepareHooks,
) -> Result<(TransactionId, PathBuf, String), TransactionPrepareError> {
    for _ in 0..MAX_TRANSACTION_ID_ATTEMPTS {
        let (candidate, created_at_utc) = TransactionId::new_candidate(project.fingerprint())
            .map_err(TransactionPrepareError::Model)?;
        let directory = transactions_root.join(candidate.as_str());
        #[cfg(test)]
        check_prepare_hook!(hooks, PrepareFailPoint::CreateTransactionDirectory, None).map_err(
            |source| {
                TransactionPrepareError::failed(
                    PrepareStage::CreateTransactionDirectory,
                    project,
                    Some(&candidate),
                    None,
                    PrepareFailureSource::Io(source),
                )
            },
        )?;
        match fs::create_dir(&directory) {
            Ok(()) => {
                sync_directory(transactions_root).map_err(|source| {
                    TransactionPrepareError::failed(
                        PrepareStage::SyncDirectory,
                        project,
                        Some(&candidate),
                        None,
                        PrepareFailureSource::Io(source),
                    )
                })?;
                return Ok((candidate, directory, created_at_utc));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(TransactionPrepareError::failed(
                    PrepareStage::CreateTransactionDirectory,
                    project,
                    Some(&candidate),
                    None,
                    PrepareFailureSource::Io(source),
                ));
            }
        }
    }
    Err(TransactionPrepareError::failed(
        PrepareStage::CreateTransactionDirectory,
        project,
        None,
        None,
        PrepareFailureSource::Io(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not allocate a unique transaction ID",
        )),
    ))
}

fn write_artifacts(
    project: &LockedProject<'_>,
    transaction_id: &TransactionId,
    staged_directory: &Path,
    backups_directory: &Path,
    changes: Vec<ResolvedChange>,
    #[cfg(test)] hooks: &impl PrepareHooks,
) -> Result<PreparedArtifacts, TransactionPrepareError> {
    let mut operations = Vec::with_capacity(changes.len());
    for (position, change) in changes.into_iter().enumerate() {
        let index =
            u32::try_from(position).map_err(|_| TransactionPrepareError::EstimateOverflow)?;
        #[cfg(test)]
        check_prepare_hook!(hooks, PrepareFailPoint::BeforeOriginalRead, Some(index)).map_err(
            |source| {
                TransactionPrepareError::failed(
                    PrepareStage::ReadOriginal,
                    project,
                    Some(transaction_id),
                    Some(&change.target),
                    PrepareFailureSource::Io(source),
                )
            },
        )?;
        let original = read_original(project, transaction_id, &change)?;
        let staged_path = staged_artifact_path(index);
        let staged_absolute = staged_directory.join(format!("{index:06}.json"));
        write_new_synced(
            &staged_absolute,
            change.payload.bytes(),
            #[cfg(test)]
            hooks,
            #[cfg(test)]
            PrepareFailPoint::StagedCreate,
            #[cfg(test)]
            PrepareFailPoint::StagedWrite,
            #[cfg(test)]
            PrepareFailPoint::StagedSync,
            #[cfg(test)]
            Some(index),
        )
        .map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::WriteStaged,
                project,
                Some(transaction_id),
                Some(&change.target),
                PrepareFailureSource::Io(source),
            )
        })?;
        verify_exact_artifact(&staged_absolute, change.payload.bytes()).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::WriteStaged,
                project,
                Some(transaction_id),
                Some(&change.target),
                source,
            )
        })?;

        let staged_size = u64::try_from(change.payload.bytes().len())
            .map_err(|_| TransactionPrepareError::EstimateOverflow)?;
        let staged_sha256 = sha256(change.payload.bytes());
        let staged_schema_version =
            managed_schema_version(change.payload.bytes()).map_err(|source| {
                TransactionPrepareError::failed(
                    PrepareStage::WriteStaged,
                    project,
                    Some(transaction_id),
                    Some(&change.target),
                    PrepareFailureSource::Json(source),
                )
            })?;
        let (
            backup_path,
            original_existed,
            original_size,
            original_sha256,
            original_schema_version,
        ) = match original {
            Some(bytes) => {
                let relative = backup_artifact_path(index);
                let absolute = backups_directory.join(format!("{index:06}.json"));
                write_new_synced(
                    &absolute,
                    &bytes,
                    #[cfg(test)]
                    hooks,
                    #[cfg(test)]
                    PrepareFailPoint::BackupCreate,
                    #[cfg(test)]
                    PrepareFailPoint::BackupWrite,
                    #[cfg(test)]
                    PrepareFailPoint::BackupSync,
                    #[cfg(test)]
                    Some(index),
                )
                .map_err(|source| {
                    TransactionPrepareError::failed(
                        PrepareStage::WriteBackup,
                        project,
                        Some(transaction_id),
                        Some(&change.target),
                        PrepareFailureSource::Io(source),
                    )
                })?;
                verify_exact_artifact(&absolute, &bytes).map_err(|source| {
                    TransactionPrepareError::failed(
                        PrepareStage::WriteBackup,
                        project,
                        Some(transaction_id),
                        Some(&change.target),
                        source,
                    )
                })?;
                let size = u64::try_from(bytes.len())
                    .map_err(|_| TransactionPrepareError::EstimateOverflow)?;
                let schema_version = managed_schema_version(&bytes).map_err(|source| {
                    TransactionPrepareError::failed(
                        PrepareStage::WriteBackup,
                        project,
                        Some(transaction_id),
                        Some(&change.target),
                        PrepareFailureSource::Json(source),
                    )
                })?;
                (
                    Some(relative),
                    true,
                    Some(size),
                    Some(sha256(&bytes)),
                    Some(schema_version),
                )
            }
            None => (None, false, None, None, None),
        };
        operations.push(TransactionOperation {
            index,
            target_path: change.target,
            staged_path,
            backup_path,
            original_existed,
            original_size,
            original_sha256,
            staged_size,
            staged_sha256,
            staged_schema_version: Some(staged_schema_version),
            original_schema_version,
        });
    }
    sync_directory(staged_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::SyncDirectory,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    sync_directory(backups_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::SyncDirectory,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    Ok(PreparedArtifacts { operations })
}

/// 일반 저장의 schema 불변식은 transaction directory를 만들기 전에 먼저 검사한다.
/// backup 직전의 두 번째 검사는 이 검사 뒤 원본이 바뀌는 경쟁을 차단한다.
fn precheck_schema_transitions(
    project: &LockedProject<'_>,
    changes: &mut [ResolvedChange],
) -> Result<(), TransactionPrepareError> {
    for change in changes {
        if let PlannedPayload::Canonical { write, directory } = &change.payload {
            let bytes = write.read_original(project, directory).map_err(|source| {
                TransactionPrepareError::failed(
                    PrepareStage::ValidateSchemaTransition,
                    project,
                    None,
                    Some(&change.target),
                    PrepareFailureSource::Artifact(source),
                )
            })?;
            if let Some(bytes) = bytes {
                change.original_sha256_snapshot = Some(sha256(&bytes));
                change.original_schema_snapshot =
                    Some(managed_schema_version(&bytes).map_err(|source| {
                        TransactionPrepareError::failed(
                            PrepareStage::ValidateSchemaTransition,
                            project,
                            None,
                            Some(&change.target),
                            PrepareFailureSource::Json(source),
                        )
                    })?);
                validate_schema_transition(change, &bytes).map_err(|source| {
                    TransactionPrepareError::failed(
                        PrepareStage::ValidateSchemaTransition,
                        project,
                        None,
                        Some(&change.target),
                        source,
                    )
                })?;
                change.original_size_snapshot = Some(
                    u64::try_from(bytes.len())
                        .map_err(|_| TransactionPrepareError::EstimateOverflow)?,
                );
            }
            continue;
        }
        let mut file = match open_existing_project_file(project.canonical_root(), &change.target) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if change.payload.expected_migration().is_some() {
                    return Err(TransactionPrepareError::failed(
                        PrepareStage::ValidateSchemaTransition,
                        project,
                        None,
                        Some(&change.target),
                        PrepareFailureSource::ExpectedOriginalMismatch(io::Error::new(
                            io::ErrorKind::NotFound,
                            "migration source disappeared before transaction allocation",
                        )),
                    ));
                }
                continue;
            }
            Err(source) => {
                return Err(TransactionPrepareError::failed(
                    PrepareStage::ValidateSchemaTransition,
                    project,
                    None,
                    Some(&change.target),
                    PrepareFailureSource::Io(source),
                ));
            }
        };
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::ValidateSchemaTransition,
                project,
                None,
                Some(&change.target),
                PrepareFailureSource::Io(source),
            )
        })?;
        validate_schema_transition(change, &bytes).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::ValidateSchemaTransition,
                project,
                None,
                Some(&change.target),
                source,
            )
        })?;
        change.original_size_snapshot = Some(
            u64::try_from(bytes.len()).map_err(|_| TransactionPrepareError::EstimateOverflow)?,
        );
    }
    Ok(())
}

fn read_original(
    project: &LockedProject<'_>,
    transaction_id: &TransactionId,
    change: &ResolvedChange,
) -> Result<Option<Vec<u8>>, TransactionPrepareError> {
    if let PlannedPayload::Canonical { write, directory } = &change.payload {
        let bytes = write.read_original(project, directory).map_err(|source| {
            TransactionPrepareError::failed(
                PrepareStage::ReadOriginal,
                project,
                Some(transaction_id),
                Some(&change.target),
                PrepareFailureSource::Artifact(source),
            )
        })?;
        if let Some(bytes) = &bytes {
            validate_schema_transition(change, bytes).map_err(|source| {
                TransactionPrepareError::failed(
                    PrepareStage::ReadOriginal,
                    project,
                    Some(transaction_id),
                    Some(&change.target),
                    source,
                )
            })?;
        }
        return Ok(bytes);
    }
    let mut file = match open_existing_project_file(project.canonical_root(), &change.target) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            if change.payload.expected_migration().is_some() {
                return Err(TransactionPrepareError::failed(
                    PrepareStage::ReadOriginal,
                    project,
                    Some(transaction_id),
                    Some(&change.target),
                    PrepareFailureSource::ExpectedOriginalMismatch(io::Error::new(
                        io::ErrorKind::NotFound,
                        "migration source disappeared before backup",
                    )),
                ));
            }
            return Ok(None);
        }
        Err(source) => {
            return Err(TransactionPrepareError::failed(
                PrepareStage::ReadOriginal,
                project,
                Some(transaction_id),
                Some(&change.target),
                PrepareFailureSource::Io(source),
            ));
        }
        Ok(file) => file,
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::ReadOriginal,
            project,
            Some(transaction_id),
            Some(&change.target),
            PrepareFailureSource::Io(source),
        )
    })?;
    validate_schema_transition(change, &bytes).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::ReadOriginal,
            project,
            Some(transaction_id),
            Some(&change.target),
            source,
        )
    })?;
    Ok(Some(bytes))
}

fn validate_schema_transition(
    change: &ResolvedChange,
    original: &[u8],
) -> Result<(), PrepareFailureSource> {
    let staged_schema =
        managed_schema_version(change.payload.bytes()).map_err(PrepareFailureSource::Json)?;
    if let Some(expected) = change.payload.expected_migration() {
        let original_schema = strict_schema_version(original).map_err(|_| {
            PrepareFailureSource::ExpectedOriginalMismatch(io::Error::new(
                io::ErrorKind::InvalidData,
                "migration source is no longer strict managed JSON",
            ))
        })?;
        if !expected.matches(original, original_schema) {
            return Err(PrepareFailureSource::ExpectedOriginalMismatch(
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "migration source changed before backup",
                ),
            ));
        }
        if original_schema >= staged_schema {
            return Err(PrepareFailureSource::SchemaTransitionMismatch(
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "migration schemaVersion must strictly increase",
                ),
            ));
        }
    } else {
        let original_schema =
            managed_schema_version(original).map_err(PrepareFailureSource::Json)?;
        let guide_upgrade = matches!(&change.payload, PlannedPayload::Canonical { write, .. } if write.allows_schema_transition(original_schema, staged_schema));
        if original_schema != staged_schema && !guide_upgrade {
            return Err(PrepareFailureSource::SchemaTransitionMismatch(
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "ordinary transaction schemaVersion changed",
                ),
            ));
        }
    }
    Ok(())
}

fn write_new_synced(
    path: &Path,
    bytes: &[u8],
    #[cfg(test)] hooks: &impl PrepareHooks,
    #[cfg(test)] create_point: PrepareFailPoint,
    #[cfg(test)] write_point: PrepareFailPoint,
    #[cfg(test)] sync_point: PrepareFailPoint,
    #[cfg(test)] operation: Option<u32>,
) -> io::Result<()> {
    #[cfg(test)]
    check_prepare_hook!(hooks, create_point, operation)?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    #[cfg(test)]
    check_prepare_hook!(hooks, write_point, operation)?;
    file.write_all(bytes)?;
    file.flush()?;
    #[cfg(test)]
    check_prepare_hook!(hooks, sync_point, operation)?;
    file.sync_all()?;
    drop(file);
    Ok(())
}

fn verify_exact_artifact(path: &Path, expected: &[u8]) -> Result<(), PrepareFailureSource> {
    let actual = fs::read(path).map_err(PrepareFailureSource::Io)?;
    if actual.len() != expected.len() || sha256(&actual) != sha256(expected) || actual != expected {
        return Err(PrepareFailureSource::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "artifact bytes, size, or SHA-256 differ",
        )));
    }
    validate_managed_json(&actual).map_err(PrepareFailureSource::Json)
}

fn verify_manifest(
    project: &LockedProject<'_>,
    transaction_id: &TransactionId,
    manifest_path: &Path,
    expected_bytes: &[u8],
    expected: &TransactionManifest,
    transaction_directory: &Path,
    #[cfg(test)] hooks: &impl PrepareHooks,
) -> Result<(), TransactionPrepareError> {
    #[cfg(test)]
    check_prepare_hook!(hooks, PrepareFailPoint::ManifestVerify, None).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::VerifyManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    let actual = fs::read(manifest_path).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::VerifyManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })?;
    if actual != expected_bytes {
        return Err(TransactionPrepareError::failed(
            PrepareStage::VerifyManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "manifest bytes differ from deterministic JSON",
            )),
        ));
    }
    let decoded: TransactionManifest = serde_json::from_slice(&actual).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::VerifyManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(io::Error::new(io::ErrorKind::InvalidData, source)),
        )
    })?;
    decoded.validate().map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::VerifyManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Model(source),
        )
    })?;
    if &decoded != expected {
        return Err(TransactionPrepareError::failed(
            PrepareStage::VerifyManifest,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "manifest model differs after round trip",
            )),
        ));
    }
    for operation in &decoded.operations {
        let staged = transaction_directory.join(&operation.staged_path);
        verify_recorded_artifact(&staged, operation.staged_size, &operation.staged_sha256)
            .map_err(|source| {
                TransactionPrepareError::failed(
                    PrepareStage::VerifyManifest,
                    project,
                    Some(transaction_id),
                    Some(&operation.target_path),
                    source,
                )
            })?;
        if let (Some(backup_path), Some(size), Some(hash)) = (
            operation.backup_path.as_deref(),
            operation.original_size,
            operation.original_sha256.as_deref(),
        ) {
            verify_recorded_artifact(&transaction_directory.join(backup_path), size, hash)
                .map_err(|source| {
                    TransactionPrepareError::failed(
                        PrepareStage::VerifyManifest,
                        project,
                        Some(transaction_id),
                        Some(&operation.target_path),
                        source,
                    )
                })?;
        }
    }
    sync_directory(transaction_directory).map_err(|source| {
        TransactionPrepareError::failed(
            PrepareStage::SyncDirectory,
            project,
            Some(transaction_id),
            None,
            PrepareFailureSource::Io(source),
        )
    })
}

fn verify_recorded_artifact(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), PrepareFailureSource> {
    let bytes = fs::read(path).map_err(PrepareFailureSource::Io)?;
    if u64::try_from(bytes.len()).ok() != Some(expected_size)
        || !is_lowercase_sha256(expected_sha256)
        || sha256(&bytes) != expected_sha256
    {
        return Err(PrepareFailureSource::Io(io::Error::new(
            io::ErrorKind::InvalidData,
            "artifact does not match manifest size or SHA-256",
        )));
    }
    validate_managed_json(&bytes).map_err(PrepareFailureSource::Json)
}

pub(super) fn validate_managed_json(bytes: &[u8]) -> Result<(), ManagedJsonError> {
    managed_schema_version(bytes).map(|_| ())
}

pub(super) fn managed_schema_version(bytes: &[u8]) -> Result<SchemaVersion, ManagedJsonError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(ManagedJsonError::Parse)?;
    if !value.is_object() {
        return Err(ManagedJsonError::NotObject);
    }
    serde_json::from_value::<SchemaHeader>(value)
        .map_err(ManagedJsonError::InvalidSchema)
        .map(|header| header.schema_version)
}

pub(super) fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(64);
    for byte in digest {
        value.push(char::from(DIGITS[usize::from(byte >> 4)]));
        value.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
pub(super) fn checked_estimate(
    staged_bytes: u64,
    backup_bytes: u64,
    manifest_bytes: u64,
    preparing_state_bytes: u64,
    prepared_state_bytes: u64,
) -> Result<u64, TransactionPrepareError> {
    checked_sum([
        staged_bytes,
        backup_bytes,
        manifest_bytes,
        preparing_state_bytes,
        prepared_state_bytes,
        TRANSACTION_METADATA_SAFETY_BYTES,
    ])
}

fn checked_sum(values: impl IntoIterator<Item = u64>) -> Result<u64, TransactionPrepareError> {
    values.into_iter().try_fold(0_u64, |total, value| {
        total
            .checked_add(value)
            .ok_or(TransactionPrepareError::EstimateOverflow)
    })
}

#[cfg(unix)]
pub(super) fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(windows)]
pub(super) fn sync_directory(_path: &Path) -> io::Result<()> {
    // Rust std에는 Windows 디렉터리 엔트리를 동기화하는 이식성 있는 API가 없다.
    // 개별 파일은 sync_all하지만 부모 엔트리의 전원 장애 내구성은 완전하지 않다.
    Ok(())
}

#[cfg(not(any(unix, windows)))]
pub(super) fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}
