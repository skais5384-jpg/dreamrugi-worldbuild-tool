use std::{error::Error, fmt, io::Read};

use super::collaboration_lock::{WritePermit, WritePermitError};
use super::migration::{strict_schema_version, MigrationRegistry};
use super::migration_batch::{
    preflight_migration_batch, MigrationBatchDecision, MigrationBatchError, MigrationBatchInput,
    MigrationByteEstimate,
};
use super::project_file::open_existing_project_file;
use super::project_relative_path::ProjectRelativePath;
use super::schema::SchemaVersion;
use super::storage_estimate::StorageAdmission;
use super::transaction::{
    CommitOutcome, CommitResultState, LockedProject, PreparedTransaction, TransactionCommitError,
    TransactionId, TransactionPlan, TransactionPrepareError,
};

pub(crate) struct MigrationPrepareRequest<'steps> {
    pub(crate) targets: Vec<ProjectRelativePath>,
    pub(crate) registry: MigrationRegistry<'steps>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MigrationVersionTransition {
    pub(crate) source: SchemaVersion,
    pub(crate) target: SchemaVersion,
}

pub(crate) struct PreparedMigrationTransaction<'project, 'lock, 'guard> {
    prepared: PreparedTransaction<'project, 'lock, 'guard>,
    targets: Vec<ProjectRelativePath>,
    transitions: Vec<MigrationVersionTransition>,
    preflight_estimate: MigrationByteEstimate,
}

impl<'project, 'lock, 'guard> PreparedMigrationTransaction<'project, 'lock, 'guard> {
    pub(crate) fn transaction_id(&self) -> &TransactionId {
        self.prepared.transaction_id()
    }

    pub(crate) fn project_fingerprint(&self) -> &str {
        self.prepared.project_fingerprint()
    }

    pub(crate) fn targets(&self) -> &[ProjectRelativePath] {
        &self.targets
    }

    pub(crate) fn transitions(&self) -> &[MigrationVersionTransition] {
        &self.transitions
    }

    pub(crate) fn preflight_estimate(&self) -> MigrationByteEstimate {
        self.preflight_estimate
    }

    /// transaction estimate는 실제 manifest/state 크기를 포함하므로 preflight의
    /// 고정 metadata allowance와 의미가 다르다. M1-9가 둘을 free-space 정책에 연결한다.
    pub(crate) fn transaction_estimated_required_bytes(&self) -> u64 {
        self.prepared.estimated_required_bytes()
    }

    pub(crate) fn storage_admission(&self) -> StorageAdmission {
        self.prepared.storage_admission()
    }

    /// migration wrapper만 commit 진입점을 소유한다. 이미 durable하게 준비된 bytes를
    /// transaction 엔진에 그대로 넘기며 migration registry를 다시 실행하지 않는다.
    pub(crate) fn commit(self) -> MigrationCommitReport {
        #[cfg(test)]
        {
            self.commit_using(|prepared| {
                super::transaction::test_support::commit_with_configured_test_hooks(prepared)
            })
        }
        #[cfg(not(test))]
        {
            self.commit_using(PreparedTransaction::commit)
        }
    }

    fn commit_using(
        self,
        commit: impl FnOnce(
            PreparedTransaction<'project, 'lock, 'guard>,
        ) -> Result<CommitOutcome, TransactionCommitError>,
    ) -> MigrationCommitReport {
        let Self {
            prepared,
            targets,
            transitions,
            preflight_estimate: _,
        } = self;
        let transaction_id = prepared.transaction_id().clone();
        let project_fingerprint = prepared.project_fingerprint().to_owned();
        let transaction_result = commit(prepared);
        let outcome = match transaction_result {
            Ok(transaction) => match transaction.result_state() {
                CommitResultState::Committed | CommitResultState::CommittedCleanupFailed => {
                    MigrationCommitOutcome::Committed { transaction }
                }
                CommitResultState::RolledBack | CommitResultState::RolledBackCleanupFailed => {
                    MigrationCommitOutcome::RolledBack { transaction }
                }
                CommitResultState::NotApplied | CommitResultState::RecoveryRequired => {
                    unreachable!("successful transaction outcome has an error-only state")
                }
            },
            Err(transaction) => match transaction.result_state() {
                CommitResultState::RecoveryRequired => {
                    MigrationCommitOutcome::RecoveryRequired { transaction }
                }
                CommitResultState::NotApplied => {
                    MigrationCommitOutcome::CommitFailed { transaction }
                }
                CommitResultState::Committed
                | CommitResultState::CommittedCleanupFailed
                | CommitResultState::RolledBack
                | CommitResultState::RolledBackCleanupFailed => {
                    unreachable!("transaction error has a successful outcome-only state")
                }
            },
        };
        MigrationCommitReport {
            transaction_id,
            project_fingerprint,
            targets,
            transitions,
            outcome,
        }
    }
}

#[derive(Debug)]
pub(crate) enum MigrationCommitOutcome {
    Committed { transaction: CommitOutcome },
    RolledBack { transaction: CommitOutcome },
    RecoveryRequired { transaction: TransactionCommitError },
    CommitFailed { transaction: TransactionCommitError },
}

impl MigrationCommitOutcome {
    pub(crate) fn result_state(&self) -> CommitResultState {
        match self {
            Self::Committed { transaction } | Self::RolledBack { transaction } => {
                transaction.result_state()
            }
            Self::RecoveryRequired { transaction } | Self::CommitFailed { transaction } => {
                transaction.result_state()
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct MigrationCommitReport {
    pub(crate) transaction_id: TransactionId,
    pub(crate) project_fingerprint: String,
    pub(crate) targets: Vec<ProjectRelativePath>,
    pub(crate) transitions: Vec<MigrationVersionTransition>,
    pub(crate) outcome: MigrationCommitOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationPrepareStage {
    InvalidTargetSet,
    PermitValidation,
    SourcePathResolution,
    SourceMetadataValidation,
    SourceRead,
    MigrationPreflight,
    MigrationNoLongerRequired,
    BatchInvariantValidation,
    ExpectedOriginalPrecheck,
    SourceChangedBeforeArtifacts,
    TransactionPlanConstruction,
    TransactionPrepare,
    SourceChangedDuringBackup,
}

pub(crate) struct MigrationPrepareError {
    context: Box<MigrationPrepareErrorContext>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

struct MigrationPrepareErrorContext {
    stage: MigrationPrepareStage,
    target: Option<ProjectRelativePath>,
    project_fingerprint: String,
    session_id: String,
    detail: &'static str,
    cleanup_failed: bool,
}

impl MigrationPrepareError {
    pub(crate) fn stage(&self) -> MigrationPrepareStage {
        self.context.stage
    }

    pub(crate) fn cleanup_failed(&self) -> bool {
        self.context.cleanup_failed
    }

    fn new(
        stage: MigrationPrepareStage,
        project: &LockedProject<'_>,
        session_id: String,
        target: Option<ProjectRelativePath>,
        detail: &'static str,
    ) -> Self {
        Self {
            context: Box::new(MigrationPrepareErrorContext {
                stage,
                target,
                project_fingerprint: project.fingerprint().to_owned(),
                session_id,
                detail,
                cleanup_failed: false,
            }),
            source: None,
        }
    }

    fn sourced(
        stage: MigrationPrepareStage,
        project: &LockedProject<'_>,
        session_id: String,
        target: Option<ProjectRelativePath>,
        detail: &'static str,
        source: impl Error + Send + Sync + 'static,
    ) -> Self {
        let mut result = Self::new(stage, project, session_id, target, detail);
        result.source = Some(Box::new(source));
        result
    }
}

impl fmt::Debug for MigrationPrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MigrationPrepareError")
            .field("stage", &self.context.stage)
            .field("target", &self.context.target)
            .field("project_fingerprint", &self.context.project_fingerprint)
            .field("session_id", &self.context.session_id)
            .field("detail", &self.context.detail)
            .field("cleanup_failed", &self.context.cleanup_failed)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl fmt::Display for MigrationPrepareError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "migration prepare failed at {:?} for project {}",
            self.context.stage, self.context.project_fingerprint
        )?;
        if let Some(target) = &self.context.target {
            write!(formatter, ", target '{target}'")?;
        }
        write!(formatter, ": {}", self.context.detail)?;
        if self.context.cleanup_failed {
            formatter.write_str("; transaction cleanup also failed")?;
        }
        Ok(())
    }
}

impl Error for MigrationPrepareError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

pub(crate) fn prepare_migration<'project, 'lock, 'guard>(
    request: MigrationPrepareRequest<'_>,
    project: &'project LockedProject<'lock>,
    permit: WritePermit<'guard>,
) -> Result<PreparedMigrationTransaction<'project, 'lock, 'guard>, MigrationPrepareError> {
    prepare_migration_internal(
        request,
        project,
        permit,
        #[cfg(test)]
        &NoMigrationPrepareHooks,
    )
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigrationPreparePoint {
    AfterInitialRead,
    BeforeExpectedOriginalPrecheck,
    BeforeBackupRead(u32),
    TransactionCleanup,
}

#[cfg(test)]
trait MigrationPrepareHooks {
    fn check(&self, point: MigrationPreparePoint) -> std::io::Result<()>;
}

#[cfg(test)]
struct NoMigrationPrepareHooks;

#[cfg(test)]
impl MigrationPrepareHooks for NoMigrationPrepareHooks {
    fn check(&self, _point: MigrationPreparePoint) -> std::io::Result<()> {
        Ok(())
    }
}

fn prepare_migration_internal<'project, 'lock, 'guard>(
    mut request: MigrationPrepareRequest<'_>,
    project: &'project LockedProject<'lock>,
    mut permit: WritePermit<'guard>,
    #[cfg(test)] hooks: &impl MigrationPrepareHooks,
) -> Result<PreparedMigrationTransaction<'project, 'lock, 'guard>, MigrationPrepareError> {
    let session_id = permit.session_id().to_string();
    request.targets.sort();
    if request.targets.is_empty() || request.targets.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(MigrationPrepareError::new(
            MigrationPrepareStage::InvalidTargetSet,
            project,
            session_id,
            None,
            "migration targets must be non-empty and unique",
        ));
    }
    permit
        .validate_for(project.fingerprint(), &request.targets)
        .map_err(|source| permit_error(project, session_id.clone(), source))?;

    let mut originals = Vec::with_capacity(request.targets.len());
    for target in &request.targets {
        originals.push((
            target.clone(),
            read_existing_source(project, target, &session_id)?,
        ));
    }
    #[cfg(test)]
    hooks
        .check(MigrationPreparePoint::AfterInitialRead)
        .map_err(|source| {
            MigrationPrepareError::sourced(
                MigrationPrepareStage::SourceRead,
                project,
                session_id.clone(),
                None,
                "test hook failed after source read",
                source,
            )
        })?;

    let inputs = originals
        .iter()
        .map(|(target, bytes)| MigrationBatchInput::new(target.clone(), bytes.clone()))
        .collect();
    let batch = match preflight_migration_batch(inputs, request.registry) {
        Ok(MigrationBatchDecision::Prepared(batch)) => batch,
        Ok(MigrationBatchDecision::NoMigrationRequired) => {
            return Err(MigrationPrepareError::new(
                MigrationPrepareStage::MigrationNoLongerRequired,
                project,
                session_id,
                None,
                "one or more locked targets no longer require migration",
            ));
        }
        Err(source) => return Err(batch_error(project, session_id, source)),
    };
    match compare_migrated_targets(
        batch.entries().iter().map(|entry| entry.target()),
        &request.targets,
    ) {
        MigratedTargetComparison::Exact => {}
        MigratedTargetComparison::Missing => {
            return Err(MigrationPrepareError::new(
                MigrationPrepareStage::MigrationNoLongerRequired,
                project,
                session_id,
                None,
                "one or more locked targets no longer require migration",
            ));
        }
        MigratedTargetComparison::Mismatch => {
            return Err(MigrationPrepareError::new(
                MigrationPrepareStage::BatchInvariantValidation,
                project,
                session_id,
                None,
                "migration batch target set differs from the locked target set",
            ));
        }
    }

    #[cfg(test)]
    hooks
        .check(MigrationPreparePoint::BeforeExpectedOriginalPrecheck)
        .map_err(|source| {
            MigrationPrepareError::sourced(
                MigrationPrepareStage::ExpectedOriginalPrecheck,
                project,
                session_id.clone(),
                None,
                "test hook failed before expected-original precheck",
                source,
            )
        })?;
    for entry in batch.entries() {
        let bytes =
            read_existing_source(project, entry.target(), &session_id).map_err(|source| {
                MigrationPrepareError::sourced(
                    MigrationPrepareStage::SourceChangedBeforeArtifacts,
                    project,
                    session_id.clone(),
                    Some(entry.target().clone()),
                    "source could not be safely reread before artifacts",
                    source,
                )
            })?;
        let reread_version = strict_schema_version(&bytes).map_err(|source| {
            MigrationPrepareError::sourced(
                MigrationPrepareStage::SourceChangedBeforeArtifacts,
                project,
                session_id.clone(),
                Some(entry.target().clone()),
                "source is no longer strict managed JSON",
                source,
            )
        })?;
        if !entry.expected_original().matches(&bytes, reread_version) {
            return Err(MigrationPrepareError::new(
                MigrationPrepareStage::SourceChangedBeforeArtifacts,
                project,
                session_id,
                Some(entry.target().clone()),
                "source differs from the locked migration snapshot",
            ));
        }
    }

    let preflight_estimate = batch.byte_estimate();
    let mut plan = TransactionPlan::new();
    plan.require_minimum_storage(preflight_estimate.total_bytes());
    let mut transitions = Vec::with_capacity(batch.entries().len());
    for entry in batch.into_entries() {
        let transition = plan.add_migrated_entry(entry).map_err(|source| {
            MigrationPrepareError::sourced(
                MigrationPrepareStage::TransactionPlanConstruction,
                project,
                session_id.clone(),
                None,
                "validated migration could not enter the transaction plan",
                source,
            )
        })?;
        transitions.push(MigrationVersionTransition {
            source: transition.0,
            target: transition.1,
        });
    }
    #[cfg(test)]
    let prepared = {
        let adapter = TransactionHookAdapter { hooks };
        super::transaction::prepare_with_hooks(plan, project, permit, &adapter)
    };
    #[cfg(not(test))]
    let prepared = plan.prepare(project, permit);
    let prepared = prepared.map_err(|source| transaction_error(project, session_id, source))?;
    Ok(PreparedMigrationTransaction {
        prepared,
        targets: request.targets,
        transitions,
        preflight_estimate,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MigratedTargetComparison {
    Exact,
    Missing,
    Mismatch,
}

fn compare_migrated_targets<'a>(
    mut migrated: impl ExactSizeIterator<Item = &'a ProjectRelativePath>,
    requested: &[ProjectRelativePath],
) -> MigratedTargetComparison {
    let migrated_len = migrated.len();
    if migrated_len == requested.len() {
        return if migrated.eq(requested.iter()) {
            MigratedTargetComparison::Exact
        } else {
            MigratedTargetComparison::Mismatch
        };
    }
    if migrated_len > requested.len() {
        return MigratedTargetComparison::Mismatch;
    }
    let mut requested = requested.iter();
    if migrated.all(|target| requested.any(|expected| expected == target)) {
        MigratedTargetComparison::Missing
    } else {
        MigratedTargetComparison::Mismatch
    }
}

#[cfg(test)]
fn prepare_migration_with_hooks<'project, 'lock, 'guard, H>(
    request: MigrationPrepareRequest<'_>,
    project: &'project LockedProject<'lock>,
    permit: WritePermit<'guard>,
    hooks: &H,
) -> Result<PreparedMigrationTransaction<'project, 'lock, 'guard>, MigrationPrepareError>
where
    H: MigrationPrepareHooks,
{
    prepare_migration_internal(request, project, permit, hooks)
}

fn permit_error(
    project: &LockedProject<'_>,
    session_id: String,
    source: WritePermitError,
) -> MigrationPrepareError {
    MigrationPrepareError::sourced(
        MigrationPrepareStage::PermitValidation,
        project,
        session_id,
        None,
        "exact migration write permit was rejected",
        source,
    )
}

fn batch_error(
    project: &LockedProject<'_>,
    session_id: String,
    source: MigrationBatchError,
) -> MigrationPrepareError {
    MigrationPrepareError::sourced(
        MigrationPrepareStage::MigrationPreflight,
        project,
        session_id,
        None,
        "locked source migration preflight failed",
        source,
    )
}

fn transaction_error(
    project: &LockedProject<'_>,
    session_id: String,
    source: TransactionPrepareError,
) -> MigrationPrepareError {
    let stage = if source.expected_original_mismatch() {
        MigrationPrepareStage::SourceChangedDuringBackup
    } else {
        MigrationPrepareStage::TransactionPrepare
    };
    let cleanup_failed = source.cleanup_error().is_some();
    let mut error = MigrationPrepareError::sourced(
        stage,
        project,
        session_id,
        None,
        "durable migration transaction prepare failed",
        source,
    );
    error.context.cleanup_failed = cleanup_failed;
    error
}

fn read_existing_source(
    project: &LockedProject<'_>,
    target: &ProjectRelativePath,
    session_id: &str,
) -> Result<Vec<u8>, MigrationPrepareError> {
    let mut file =
        open_existing_project_file(project.canonical_root(), target).map_err(|source| {
            MigrationPrepareError::sourced(
                if source.kind() == std::io::ErrorKind::NotFound {
                    MigrationPrepareStage::SourceMetadataValidation
                } else if source.kind() == std::io::ErrorKind::PermissionDenied {
                    MigrationPrepareStage::SourcePathResolution
                } else {
                    MigrationPrepareStage::SourceRead
                },
                project,
                session_id.to_owned(),
                Some(target.clone()),
                "could not safely open migration source",
                source,
            )
        })?;
    // 현재 schema에는 파일별 크기 상한이 없다. 무제한 할당 정책은 M1-9/상위 schema의 남은 한계다.
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| {
        MigrationPrepareError::sourced(
            MigrationPrepareStage::SourceRead,
            project,
            session_id.to_owned(),
            Some(target.clone()),
            "could not read migration source",
            source,
        )
    })?;
    Ok(bytes)
}

#[cfg(test)]
struct TransactionHookAdapter<'a, H> {
    hooks: &'a H,
}

#[cfg(test)]
impl<H: MigrationPrepareHooks> super::transaction::PrepareHooks for TransactionHookAdapter<'_, H> {
    fn check(
        &self,
        point: super::transaction::PrepareFailPoint,
        operation: Option<u32>,
    ) -> std::io::Result<()> {
        if point == super::transaction::PrepareFailPoint::BeforeOriginalRead {
            if let Some(operation) = operation {
                return self
                    .hooks
                    .check(MigrationPreparePoint::BeforeBackupRead(operation));
            }
        }
        if point == super::transaction::PrepareFailPoint::Cleanup {
            return self.hooks.check(MigrationPreparePoint::TransactionCleanup);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::cell::Cell;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::{json, Value};
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::data::collaboration_lock::{
        HeldLock, HeldLockState, LockAcquireRequest, LockCapabilities, LockCoordinator, LockError,
        LockErrorCategory, LockOperation, LockProviderInfo, LockProviderKind, LockService,
        LockSessionId,
    };
    use crate::data::json::to_deterministic_json_bytes;
    use crate::data::migration::{production_registry, MigrationStep, MigrationStepError};
    use crate::data::migration_recovery::{
        recover_migration, MigrationRecoveryRequest, MigrationRecoveryStage,
    };
    use crate::data::project_lock::ProjectLock;
    use crate::data::storage_estimate::test_support::{with_storage_response, TestStorageResponse};
    use crate::data::storage_estimate::StorageAdmissionDecision;
    use crate::data::transaction::{
        test_support::{with_commit_failures, CommitTestPoint, RecoveryTestPoint},
        test_write_permit, CommitFailureSource, CommitStage, CommittedMarker, TransactionManifest,
        TransactionState, TransactionStateRecord,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
    type TestResult = Result<(), Box<dyn Error>>;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
                let path = std::env::temp_dir().join(format!(
                    "worldbuild-migration-prepare-{}-{count}",
                    std::process::id()
                ));
                match fs::create_dir(&path) {
                    Ok(()) => return Ok(Self(path)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "test directory collision",
            ))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn with_project<F>(test: F) -> TestResult
    where
        F: FnOnce(&Path, &LockedProject<'_>) -> TestResult,
    {
        let temp = TestDirectory::new()?;
        let root = temp.0.join("project");
        fs::create_dir(&root)?;
        fs::create_dir(root.join("data"))?;
        let lock = ProjectLock::try_acquire(&root, &temp.0.join("locks"))?;
        let project = LockedProject::bind(&lock, &root)?;
        test(&root, &project)
    }

    fn validate_version(value: &Value, expected: u64) -> Result<(), MigrationStepError> {
        if value.get("schemaVersion").and_then(Value::as_u64) == Some(expected) {
            Ok(())
        } else {
            Err(MigrationStepError::new("unexpected synthetic version"))
        }
    }
    fn validate_v1(value: &Value) -> Result<(), MigrationStepError> {
        validate_version(value, 1)
    }
    fn validate_v2(value: &Value) -> Result<(), MigrationStepError> {
        validate_version(value, 2)
    }
    fn validate_v3(value: &Value) -> Result<(), MigrationStepError> {
        validate_version(value, 3)
    }
    fn v1_to_v2(mut value: Value) -> Result<Value, MigrationStepError> {
        value["schemaVersion"] = json!(2);
        value["v2"] = json!(true);
        Ok(value)
    }
    fn v2_to_v3(mut value: Value) -> Result<Value, MigrationStepError> {
        value["schemaVersion"] = json!(3);
        value["v3"] = json!(true);
        Ok(value)
    }
    static STEPS: [MigrationStep; 2] = [
        MigrationStep::new(1, 2, v1_to_v2, validate_v1, validate_v2),
        MigrationStep::new(2, 3, v2_to_v3, validate_v2, validate_v3),
    ];
    fn registry() -> MigrationRegistry<'static> {
        MigrationRegistry::try_new(3, &STEPS).expect("synthetic registry is valid")
    }
    fn target(value: &str) -> ProjectRelativePath {
        ProjectRelativePath::parse(value).expect("test target is valid")
    }

    struct FailingValidationService {
        instance_id: u64,
        validation_calls: AtomicUsize,
        fail_at: AtomicUsize,
        failure_category: LockErrorCategory,
    }

    struct FailingHeldLock {
        instance_id: u64,
        project_fingerprint: String,
        session_id: LockSessionId,
        target: ProjectRelativePath,
    }

    impl HeldLock for FailingHeldLock {
        fn provider_kind(&self) -> LockProviderKind {
            LockProviderKind::Svn
        }
        fn provider_instance_id(&self) -> u64 {
            self.instance_id
        }
        fn project_fingerprint(&self) -> &str {
            &self.project_fingerprint
        }
        fn session_id(&self) -> &LockSessionId {
            &self.session_id
        }
        fn target(&self) -> &ProjectRelativePath {
            &self.target
        }
        fn state(&self) -> HeldLockState {
            HeldLockState::Active
        }
        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }
    }

    impl LockService for FailingValidationService {
        fn provider_info(&self) -> LockProviderInfo {
            LockProviderInfo {
                kind: LockProviderKind::Svn,
                capabilities: LockCapabilities {
                    distributed: true,
                    validation: true,
                    owner_diagnostics: false,
                    steal: false,
                },
            }
        }

        fn acquire(&self, request: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
            Ok(Box::new(FailingHeldLock {
                instance_id: self.instance_id,
                project_fingerprint: request.project_fingerprint().to_owned(),
                session_id: request.session_id().clone(),
                target: request.target().clone(),
            }))
        }

        fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
            let call = self.validation_calls.fetch_add(1, Ordering::SeqCst) + 1;
            let fail_at = self.fail_at.load(Ordering::SeqCst);
            if fail_at != 0 && call >= fail_at {
                return Err(LockError::for_held(
                    self.failure_category,
                    LockProviderKind::Svn,
                    LockOperation::Validate,
                    held.project_fingerprint(),
                    held.session_id(),
                    held.target(),
                ));
            }
            Ok(())
        }

        fn release(&self, _held: &mut dyn HeldLock) -> Result<(), LockError> {
            Ok(())
        }
    }

    fn failing_permit(
        fingerprint: &str,
        targets: Vec<ProjectRelativePath>,
        category: LockErrorCategory,
    ) -> Result<WritePermit<'static>, Box<dyn Error>> {
        let service: Arc<dyn LockService> = Arc::new(FailingValidationService {
            instance_id: TEST_COUNTER.fetch_add(1, Ordering::Relaxed) + 50_000,
            validation_calls: AtomicUsize::new(0),
            fail_at: AtomicUsize::new(2),
            failure_category: category,
        });
        let coordinator = LockCoordinator::new(service);
        let session = LockSessionId::generate()?;
        let guard = coordinator.acquire_all(fingerprint, &session, targets)?;
        Ok(guard.into_test_write_permit()?)
    }

    fn controllable_permit(
        fingerprint: &str,
        targets: Vec<ProjectRelativePath>,
        failure_category: LockErrorCategory,
    ) -> Result<(WritePermit<'static>, Arc<FailingValidationService>), Box<dyn Error>> {
        let service = Arc::new(FailingValidationService {
            instance_id: TEST_COUNTER.fetch_add(1, Ordering::Relaxed) + 60_000,
            validation_calls: AtomicUsize::new(0),
            fail_at: AtomicUsize::new(0),
            failure_category,
        });
        let coordinator = LockCoordinator::new(service.clone());
        let session = LockSessionId::generate()?;
        let guard = coordinator.acquire_all(fingerprint, &session, targets)?;
        Ok((guard.into_test_write_permit()?, service))
    }

    #[test]
    fn prepares_exact_sorted_targets_with_original_backups_and_unchanged_sources() -> TestResult {
        with_project(|root, project| {
            let a = br#"{"schemaVersion":1,"name":"a"}"#.to_vec();
            let b = br#"{"schemaVersion":2,"name":"b"}"#.to_vec();
            fs::write(root.join("data/a.json"), &a)?;
            fs::write(root.join("data/b.json"), &b)?;
            let targets = vec![target("data/b.json"), target("data/a.json")];
            let permit = test_write_permit(project.fingerprint(), {
                let mut sorted = targets.clone();
                sorted.sort();
                sorted
            });
            let prepared = prepare_migration(
                MigrationPrepareRequest {
                    targets,
                    registry: registry(),
                },
                project,
                permit,
            )?;
            assert_eq!(
                prepared.targets(),
                &[target("data/a.json"), target("data/b.json")]
            );
            assert_eq!(prepared.transitions()[0].source.get(), 1);
            assert_eq!(prepared.transitions()[1].source.get(), 2);
            assert_eq!(fs::read(root.join("data/a.json"))?, a);
            assert_eq!(fs::read(root.join("data/b.json"))?, b);
            let directory = prepared.prepared.transaction_directory();
            assert_eq!(fs::read(directory.join("backups/000000.json"))?, a);
            assert_eq!(fs::read(directory.join("backups/000001.json"))?, b);
            let expected_staged_a = to_deterministic_json_bytes(&json!({
                "name": "a",
                "schemaVersion": 3,
                "v2": true,
                "v3": true
            }))?;
            assert_eq!(
                fs::read(directory.join("staged/000000.json"))?,
                expected_staged_a
            );
            let expected_staged_b = to_deterministic_json_bytes(&json!({
                "name": "b",
                "schemaVersion": 3,
                "v3": true
            }))?;
            assert_eq!(
                fs::read(directory.join("staged/000001.json"))?,
                expected_staged_b
            );
            let manifest: TransactionManifest =
                serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
            assert_eq!(manifest.operations.len(), 2);
            assert_eq!(manifest.schema_version.get(), 1);
            for (operation, original, staged, source_version) in [
                (
                    &manifest.operations[0],
                    a.as_slice(),
                    expected_staged_a.as_slice(),
                    1,
                ),
                (
                    &manifest.operations[1],
                    b.as_slice(),
                    expected_staged_b.as_slice(),
                    2,
                ),
            ] {
                assert_eq!(operation.original_size, Some(original.len() as u64));
                assert_eq!(operation.staged_size, staged.len() as u64);
                assert_eq!(
                    operation.original_sha256.as_deref(),
                    Some(hex_sha256(original).as_str())
                );
                assert_eq!(operation.staged_sha256, hex_sha256(staged));
                assert_eq!(
                    operation.original_schema_version.map(SchemaVersion::get),
                    Some(source_version)
                );
                assert_eq!(
                    operation.staged_schema_version.map(SchemaVersion::get),
                    Some(3)
                );
            }
            assert!(
                prepared.transaction_estimated_required_bytes()
                    >= prepared.preflight_estimate().staged_bytes()
            );
            let admission = prepared.storage_admission();
            assert_eq!(admission.decision(), StorageAdmissionDecision::Admitted);
            assert_eq!(
                admission.required_peak_bytes(),
                prepared.transaction_estimated_required_bytes()
            );
            assert!(admission.required_peak_bytes() >= prepared.preflight_estimate().total_bytes());
            let state: TransactionStateRecord =
                serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
            assert_eq!(state.state, TransactionState::Prepared);
            let recovery_directory = directory.to_path_buf();
            drop(prepared);
            assert!(recovery_directory.join("staged/000000.json").is_file());
            assert!(recovery_directory.join("backups/000000.json").is_file());
            assert!(recovery_directory.join("manifest.json").is_file());
            assert!(recovery_directory.join("state.json").is_file());
            assert_eq!(fs::read(root.join("data/a.json"))?, a);
            assert_eq!(fs::read(root.join("data/b.json"))?, b);
            Ok(())
        })
    }

    #[test]
    fn prepares_one_existing_file_without_changing_it() -> TestResult {
        with_project(|root, project| {
            let original = br#"{"schemaVersion":1,"single":true}"#.to_vec();
            fs::write(root.join("data/single.json"), &original)?;
            let targets = vec![target("data/single.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let prepared = prepare_migration(
                MigrationPrepareRequest {
                    targets,
                    registry: registry(),
                },
                project,
                permit,
            )?;
            assert_eq!(prepared.targets(), &[target("data/single.json")]);
            assert_eq!(fs::read(root.join("data/single.json"))?, original);
            assert_eq!(prepared.prepared.manifest().operations.len(), 1);
            Ok(())
        })
    }

    #[test]
    fn migration_storage_denial_happens_before_artifacts_and_preserves_source() -> TestResult {
        with_project(|root, project| {
            let path = root.join("data/a.json");
            let original = br#"{"schemaVersion":1,"value":"secret"}"#.to_vec();
            fs::write(&path, &original)?;
            let targets = vec![target("data/a.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());

            let (result, records) = with_storage_response(
                TestStorageResponse::Available {
                    available_bytes: 0,
                    allocation_unit_bytes: 4096,
                },
                || {
                    prepare_migration(
                        MigrationPrepareRequest {
                            targets,
                            registry: registry(),
                        },
                        project,
                        permit,
                    )
                },
            );
            let error = result.err().ok_or("migration storage denial must fail")?;
            assert_eq!(error.stage(), MigrationPrepareStage::TransactionPrepare);
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].artifact_path, project.transactions_root());
            assert_eq!(
                records[0].target_paths,
                vec![project.canonical_root().join("data/a.json")]
            );
            assert_eq!(fs::read(&path)?, original);
            assert!(!root.join(".worldbuild").exists());
            let diagnostic = format!("{error:?}\n{error}");
            assert!(!diagnostic.contains(&root.display().to_string()));
            assert!(!diagnostic.contains("secret"));
            Ok(())
        })
    }

    fn hex_sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[test]
    fn rejects_invalid_or_inexact_target_sets_before_artifacts() -> TestResult {
        with_project(|root, project| {
            fs::write(root.join("data/a.json"), br#"{"schemaVersion":1}"#)?;
            let a = target("data/a.json");
            for targets in [Vec::new(), vec![a.clone(), target(r"data\a.json")]] {
                let permit = test_write_permit(project.fingerprint(), vec![a.clone()]);
                let error = prepare_migration(
                    MigrationPrepareRequest {
                        targets,
                        registry: registry(),
                    },
                    project,
                    permit,
                )
                .err()
                .expect("invalid set must fail");
                assert_eq!(error.stage(), MigrationPrepareStage::InvalidTargetSet);
                assert!(!root.join(".worldbuild").exists());
            }
            let permit = test_write_permit(
                project.fingerprint(),
                vec![a.clone(), target("data/b.json")],
            );
            let error = prepare_migration(
                MigrationPrepareRequest {
                    targets: vec![a],
                    registry: registry(),
                },
                project,
                permit,
            )
            .err()
            .expect("superset permit must fail");
            assert_eq!(error.stage(), MigrationPrepareStage::PermitValidation);
            assert!(!root.join(".worldbuild").exists());

            let subset_request = vec![target("data/a.json"), target("data/b.json")];
            let permit = test_write_permit(project.fingerprint(), vec![target("data/a.json")]);
            let error = prepare_migration(
                MigrationPrepareRequest {
                    targets: subset_request,
                    registry: registry(),
                },
                project,
                permit,
            )
            .err()
            .expect("subset permit must fail");
            assert_eq!(error.stage(), MigrationPrepareStage::PermitValidation);
            assert!(!root.join(".worldbuild").exists());

            let permit = test_write_permit(
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                vec![target("data/a.json")],
            );
            let error = prepare_migration(
                MigrationPrepareRequest {
                    targets: vec![target("data/a.json")],
                    registry: registry(),
                },
                project,
                permit,
            )
            .err()
            .expect("project mismatch must fail");
            assert_eq!(error.stage(), MigrationPrepareStage::PermitValidation);
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })
    }

    #[test]
    fn lost_unknown_and_invalid_permits_fail_before_source_read() -> TestResult {
        for category in [
            LockErrorCategory::LockLost,
            LockErrorCategory::LockStateUnknown,
            LockErrorCategory::LockValidationFailed,
        ] {
            with_project(|root, project| {
                // 읽기까지 진행했다면 migration preflight가 InvalidInput으로 실패할 bytes다.
                fs::write(root.join("data/a.json"), b"not-json")?;
                let targets = vec![target("data/a.json")];
                let permit = failing_permit(project.fingerprint(), targets.clone(), category)?;
                let error = prepare_migration(
                    MigrationPrepareRequest {
                        targets,
                        registry: registry(),
                    },
                    project,
                    permit,
                )
                .err()
                .expect("invalid permit must fail");
                assert_eq!(error.stage(), MigrationPrepareStage::PermitValidation);
                assert!(!root.join(".worldbuild").exists());
                assert_eq!(fs::read(root.join("data/a.json"))?, b"not-json");
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn production_registry_and_current_sources_create_no_artifacts() -> TestResult {
        with_project(|root, project| {
            fs::write(root.join("data/a.json"), br#"{"schemaVersion":1}"#)?;
            let targets = vec![target("data/a.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let error = prepare_migration(
                MigrationPrepareRequest {
                    targets,
                    registry: production_registry()?,
                },
                project,
                permit,
            )
            .err()
            .expect("production has no migration step");
            assert_eq!(
                error.stage(),
                MigrationPrepareStage::MigrationNoLongerRequired
            );
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })
    }

    #[test]
    fn mixed_current_target_is_reported_as_migration_no_longer_required() -> TestResult {
        with_project(|root, project| {
            fs::write(root.join("data/old.json"), br#"{"schemaVersion":1}"#)?;
            fs::write(root.join("data/current.json"), br#"{"schemaVersion":3}"#)?;
            let targets = vec![target("data/current.json"), target("data/old.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let error = prepare_migration(
                MigrationPrepareRequest {
                    targets,
                    registry: registry(),
                },
                project,
                permit,
            )
            .err()
            .expect("mixed current target must stop prepare");
            assert_eq!(
                error.stage(),
                MigrationPrepareStage::MigrationNoLongerRequired
            );
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })
    }

    #[test]
    fn migrated_target_comparison_rejects_missing_added_and_reordered_entries() {
        let requested = vec![target("a.json"), target("b.json")];
        let exact = [target("a.json"), target("b.json")];
        let missing = [target("a.json")];
        let foreign_shorter = [target("z.json")];
        let added = [target("a.json"), target("b.json"), target("c.json")];
        let reordered = [target("b.json"), target("a.json")];
        assert_eq!(
            compare_migrated_targets(exact.iter(), &requested),
            MigratedTargetComparison::Exact
        );
        assert_eq!(
            compare_migrated_targets(missing.iter(), &requested),
            MigratedTargetComparison::Missing
        );
        assert_eq!(
            compare_migrated_targets(foreign_shorter.iter(), &requested),
            MigratedTargetComparison::Mismatch
        );
        assert_eq!(
            compare_migrated_targets(added.iter(), &requested),
            MigratedTargetComparison::Mismatch
        );
        assert_eq!(
            compare_migrated_targets(reordered.iter(), &requested),
            MigratedTargetComparison::Mismatch
        );
    }

    #[test]
    fn rejects_missing_directory_future_and_invalid_sources_without_artifacts() -> TestResult {
        for (name, contents, directory) in [
            ("missing.json", None, false),
            ("directory.json", None, true),
            (
                "future.json",
                Some(br#"{"schemaVersion":4}"#.as_slice()),
                false,
            ),
            (
                "invalid.json",
                Some(br#"{"schemaVersion":1,"x":}"#.as_slice()),
                false,
            ),
        ] {
            with_project(|root, project| {
                let relative = target(&format!("data/{name}"));
                let path = root.join(relative.as_str());
                if directory {
                    fs::create_dir(&path)?;
                }
                if let Some(contents) = contents {
                    fs::write(&path, contents)?;
                }
                let targets = vec![relative];
                let permit = test_write_permit(project.fingerprint(), targets.clone());
                assert!(prepare_migration(
                    MigrationPrepareRequest {
                        targets,
                        registry: registry(),
                    },
                    project,
                    permit,
                )
                .is_err());
                assert!(!root.join(".worldbuild").exists());
                Ok(())
            })?;
        }
        Ok(())
    }

    struct MutationHook {
        point: MigrationPreparePoint,
        path: PathBuf,
        replacement: Vec<u8>,
        fired: Cell<bool>,
        cleanup_fails: bool,
    }

    enum PrecheckMutation {
        Remove,
        ReplaceWithDirectory,
        InvalidJson,
        #[cfg(unix)]
        SymlinkTo(PathBuf),
    }

    struct PathMutationHook {
        point: MigrationPreparePoint,
        path: PathBuf,
        mutation: PrecheckMutation,
        fired: Cell<bool>,
    }

    impl MigrationPrepareHooks for PathMutationHook {
        fn check(&self, point: MigrationPreparePoint) -> io::Result<()> {
            if point != self.point || self.fired.replace(true) {
                return Ok(());
            }
            match &self.mutation {
                PrecheckMutation::Remove => fs::remove_file(&self.path),
                PrecheckMutation::ReplaceWithDirectory => {
                    fs::remove_file(&self.path)?;
                    fs::create_dir(&self.path)
                }
                PrecheckMutation::InvalidJson => fs::write(&self.path, b"{invalid"),
                #[cfg(unix)]
                PrecheckMutation::SymlinkTo(destination) => {
                    use std::os::unix::fs::symlink;

                    fs::remove_file(&self.path)?;
                    symlink(destination, &self.path)
                }
            }
        }
    }

    #[test]
    fn precheck_path_and_strict_json_changes_keep_source_change_stage() -> TestResult {
        for mutation in [
            PrecheckMutation::Remove,
            PrecheckMutation::ReplaceWithDirectory,
            PrecheckMutation::InvalidJson,
        ] {
            with_project(|root, project| {
                let path = root.join("data/a.json");
                fs::write(&path, br#"{"schemaVersion":1,"value":"original"}"#)?;
                let targets = vec![target("data/a.json")];
                let permit = test_write_permit(project.fingerprint(), targets.clone());
                let hook = PathMutationHook {
                    point: MigrationPreparePoint::BeforeExpectedOriginalPrecheck,
                    path,
                    mutation,
                    fired: Cell::new(false),
                };
                let error = prepare_migration_with_hooks(
                    MigrationPrepareRequest {
                        targets,
                        registry: registry(),
                    },
                    project,
                    permit,
                    &hook,
                )
                .err()
                .expect("precheck path or JSON change must fail");
                assert_eq!(
                    error.stage(),
                    MigrationPrepareStage::SourceChangedBeforeArtifacts
                );
                assert!(!root.join(".worldbuild").exists());
                Ok(())
            })?;
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn precheck_symlink_escape_keeps_source_change_stage() -> TestResult {
        with_project(|root, project| {
            let path = root.join("data/a.json");
            let outside = root
                .parent()
                .ok_or("test project parent missing")?
                .join("outside.json");
            fs::write(&path, br#"{"schemaVersion":1,"value":"original"}"#)?;
            fs::write(&outside, br#"{"schemaVersion":1,"value":"outside"}"#)?;
            let targets = vec![target("data/a.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let hook = PathMutationHook {
                point: MigrationPreparePoint::BeforeExpectedOriginalPrecheck,
                path,
                mutation: PrecheckMutation::SymlinkTo(outside),
                fired: Cell::new(false),
            };
            let error = prepare_migration_with_hooks(
                MigrationPrepareRequest {
                    targets,
                    registry: registry(),
                },
                project,
                permit,
                &hook,
            )
            .err()
            .expect("precheck symlink escape must fail");
            assert_eq!(
                error.stage(),
                MigrationPrepareStage::SourceChangedBeforeArtifacts
            );
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })
    }

    #[test]
    fn backup_open_error_is_not_misclassified_as_source_mismatch() -> TestResult {
        with_project(|root, project| {
            let path = root.join("data/a.json");
            fs::write(&path, br#"{"schemaVersion":1,"value":"original"}"#)?;
            let targets = vec![target("data/a.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let hook = PathMutationHook {
                point: MigrationPreparePoint::BeforeBackupRead(0),
                path,
                mutation: PrecheckMutation::ReplaceWithDirectory,
                fired: Cell::new(false),
            };
            let error = prepare_migration_with_hooks(
                MigrationPrepareRequest {
                    targets,
                    registry: registry(),
                },
                project,
                permit,
                &hook,
            )
            .err()
            .expect("backup open failure must stop prepare");
            assert_eq!(error.stage(), MigrationPrepareStage::TransactionPrepare);
            assert!(!error.cleanup_failed());
            assert!(!root
                .join(".worldbuild/transactions")
                .read_dir()?
                .any(|_| true));
            Ok(())
        })
    }
    impl MigrationPrepareHooks for MutationHook {
        fn check(&self, point: MigrationPreparePoint) -> io::Result<()> {
            if point == self.point && !self.fired.replace(true) {
                fs::write(&self.path, &self.replacement)?;
            }
            if point == MigrationPreparePoint::TransactionCleanup && self.cleanup_fails {
                return Err(io::Error::other("synthetic cleanup failure"));
            }
            Ok(())
        }
    }

    #[test]
    fn detects_same_length_change_before_artifacts_and_keeps_external_value() -> TestResult {
        with_project(|root, project| {
            let path = root.join("data/a.json");
            fs::write(&path, br#"{"schemaVersion":1,"v":"a"}"#)?;
            let replacement = br#"{"schemaVersion":1,"v":"b"}"#.to_vec();
            let targets = vec![target("data/a.json")];
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let hook = MutationHook {
                point: MigrationPreparePoint::BeforeExpectedOriginalPrecheck,
                path: path.clone(),
                replacement: replacement.clone(),
                fired: Cell::new(false),
                cleanup_fails: false,
            };
            let error = prepare_migration_with_hooks(
                MigrationPrepareRequest {
                    targets,
                    registry: registry(),
                },
                project,
                permit,
                &hook,
            )
            .err()
            .expect("source change must fail");
            assert_eq!(
                error.stage(),
                MigrationPrepareStage::SourceChangedBeforeArtifacts
            );
            assert_eq!(fs::read(path)?, replacement);
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })
    }

    #[test]
    fn detects_first_middle_last_changes_after_initial_read_and_before_artifacts() -> TestResult {
        for point in [
            MigrationPreparePoint::AfterInitialRead,
            MigrationPreparePoint::BeforeExpectedOriginalPrecheck,
        ] {
            for changed_index in 0..3 {
                with_project(|root, project| {
                    let targets: Vec<_> = (0..3)
                        .map(|index| target(&format!("data/{index}.json")))
                        .collect();
                    for (index, item) in targets.iter().enumerate() {
                        fs::write(
                            root.join(item.as_str()),
                            format!(r#"{{"schemaVersion":1,"value":{index}}}"#),
                        )?;
                    }
                    let changed_path = root.join(targets[changed_index].as_str());
                    let replacement =
                        format!(r#"{{"schemaVersion":1,"value":{}}}"#, changed_index + 10)
                            .into_bytes();
                    let permit = test_write_permit(project.fingerprint(), targets.clone());
                    let hook = MutationHook {
                        point,
                        path: changed_path.clone(),
                        replacement: replacement.clone(),
                        fired: Cell::new(false),
                        cleanup_fails: false,
                    };
                    let error = prepare_migration_with_hooks(
                        MigrationPrepareRequest {
                            targets,
                            registry: registry(),
                        },
                        project,
                        permit,
                        &hook,
                    )
                    .err()
                    .expect("artifact-before race must fail");
                    assert_eq!(
                        error.stage(),
                        MigrationPrepareStage::SourceChangedBeforeArtifacts
                    );
                    assert_eq!(fs::read(changed_path)?, replacement);
                    assert!(!root.join(".worldbuild").exists());
                    Ok(())
                })?;
            }
        }
        Ok(())
    }

    #[test]
    fn detects_length_and_schema_only_changes_before_artifacts() -> TestResult {
        for replacement in [
            br#"{"schemaVersion":1,"value":"longer"}"#.as_slice(),
            br#"{"schemaVersion":2,"value":"a"}"#.as_slice(),
        ] {
            with_project(|root, project| {
                let path = root.join("data/a.json");
                fs::write(&path, br#"{"schemaVersion":1,"value":"a"}"#)?;
                let targets = vec![target("data/a.json")];
                let permit = test_write_permit(project.fingerprint(), targets.clone());
                let hook = MutationHook {
                    point: MigrationPreparePoint::BeforeExpectedOriginalPrecheck,
                    path: path.clone(),
                    replacement: replacement.to_vec(),
                    fired: Cell::new(false),
                    cleanup_fails: false,
                };
                let error = prepare_migration_with_hooks(
                    MigrationPrepareRequest {
                        targets,
                        registry: registry(),
                    },
                    project,
                    permit,
                    &hook,
                )
                .err()
                .expect("expected-original change must fail");
                assert_eq!(
                    error.stage(),
                    MigrationPrepareStage::SourceChangedBeforeArtifacts
                );
                assert_eq!(fs::read(path)?, replacement);
                assert!(!root.join(".worldbuild").exists());
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn detects_first_middle_last_backup_races_and_preserves_primary_error() -> TestResult {
        for operation in 0..3 {
            with_project(|root, project| {
                let targets: Vec<_> = (0..3)
                    .map(|index| target(&format!("data/{index}.json")))
                    .collect();
                for (index, item) in targets.iter().enumerate() {
                    fs::write(
                        root.join(item.as_str()),
                        format!(r#"{{"schemaVersion":1,"v":{index}}}"#),
                    )?;
                }
                let changed = br#"{"schemaVersion":1,"changed":true}"#.to_vec();
                let changed_path = root.join(targets[operation as usize].as_str());
                let permit = test_write_permit(project.fingerprint(), targets.clone());
                let hook = MutationHook {
                    point: MigrationPreparePoint::BeforeBackupRead(operation),
                    path: changed_path.clone(),
                    replacement: changed.clone(),
                    fired: Cell::new(false),
                    cleanup_fails: operation == 1,
                };
                let error = prepare_migration_with_hooks(
                    MigrationPrepareRequest {
                        targets,
                        registry: registry(),
                    },
                    project,
                    permit,
                    &hook,
                )
                .err()
                .expect("backup race must fail");
                if operation == 1 {
                    assert_eq!(
                        error.stage(),
                        MigrationPrepareStage::SourceChangedDuringBackup
                    );
                    assert!(error.cleanup_failed());
                    assert!(error.source().is_some());
                } else {
                    assert_eq!(
                        error.stage(),
                        MigrationPrepareStage::SourceChangedDuringBackup
                    );
                    assert!(!root
                        .join(".worldbuild/transactions")
                        .read_dir()?
                        .any(|_| true));
                }
                assert_eq!(fs::read(changed_path)?, changed);
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn consuming_commit_preserves_mixed_transitions_and_exact_staged_bytes() -> TestResult {
        with_project(|root, project| {
            let targets = vec![target("data/a.json"), target("data/b.json")];
            fs::write(
                root.join(targets[0].as_str()),
                br#"{"schemaVersion":1,"name":"a"}"#,
            )?;
            fs::write(
                root.join(targets[1].as_str()),
                br#"{"schemaVersion":2,"name":"b"}"#,
            )?;
            let permit = test_write_permit(project.fingerprint(), targets.clone());
            let prepared = prepare_migration(
                MigrationPrepareRequest {
                    targets: targets.clone(),
                    registry: registry(),
                },
                project,
                permit,
            )?;
            let staged = prepared
                .prepared
                .manifest()
                .operations
                .iter()
                .map(|operation| {
                    fs::read(
                        prepared
                            .prepared
                            .transaction_directory()
                            .join(&operation.staged_path),
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;

            let report = prepared.commit();
            assert_eq!(report.targets, targets);
            assert_eq!(report.project_fingerprint, project.fingerprint());
            assert_eq!(report.transitions[0].source.get(), 1);
            assert_eq!(report.transitions[0].target.get(), 3);
            assert_eq!(report.transitions[1].source.get(), 2);
            assert_eq!(report.transitions[1].target.get(), 3);
            assert_eq!(report.outcome.result_state(), CommitResultState::Committed);
            assert!(matches!(
                report.outcome,
                MigrationCommitOutcome::Committed {
                    transaction: CommitOutcome::Committed
                }
            ));
            for (target, expected) in report.targets.iter().zip(staged) {
                assert_eq!(fs::read(root.join(target.as_str()))?, expected);
                assert_eq!(strict_schema_version(&expected)?.get(), 3);
            }
            assert!(fs::read_dir(project.transactions_root())?.next().is_none());
            Ok(())
        })
    }

    #[test]
    fn commit_revalidates_same_length_schema_change_and_missing_target() -> TestResult {
        for replacement in [
            Some(br#"{"schemaVersion":1,"name":"b"}"#.as_slice()),
            Some(br#"{"schemaVersion":2,"name":"a"}"#.as_slice()),
            None,
        ] {
            with_project(|root, project| {
                let target = target("data/a.json");
                let original = br#"{"schemaVersion":1,"name":"a"}"#;
                fs::write(root.join(target.as_str()), original)?;
                let permit = test_write_permit(project.fingerprint(), vec![target.clone()]);
                let prepared = prepare_migration(
                    MigrationPrepareRequest {
                        targets: vec![target.clone()],
                        registry: registry(),
                    },
                    project,
                    permit,
                )?;
                match replacement {
                    Some(bytes) => fs::write(root.join(target.as_str()), bytes)?,
                    None => fs::remove_file(root.join(target.as_str()))?,
                }

                let report = prepared.commit();
                assert_eq!(report.outcome.result_state(), CommitResultState::NotApplied);
                assert!(matches!(
                    report.outcome,
                    MigrationCommitOutcome::CommitFailed { .. }
                ));
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn commit_preserves_rolled_back_recovery_required_and_committed_cleanup_states() -> TestResult {
        for (commit_failure, recovery_failure, expected, target_version) in [
            (
                CommitTestPoint::TargetSync,
                None,
                CommitResultState::RolledBack,
                1,
            ),
            (
                CommitTestPoint::TargetSync,
                Some(RecoveryTestPoint::BackupVerify),
                CommitResultState::RecoveryRequired,
                3,
            ),
            (
                CommitTestPoint::Cleanup,
                None,
                CommitResultState::CommittedCleanupFailed,
                3,
            ),
        ] {
            with_project(|root, project| {
                let target = target("data/a.json");
                fs::write(
                    root.join(target.as_str()),
                    br#"{"schemaVersion":1,"name":"old"}"#,
                )?;
                let prepared = prepare_migration(
                    MigrationPrepareRequest {
                        targets: vec![target.clone()],
                        registry: registry(),
                    },
                    project,
                    test_write_permit(project.fingerprint(), vec![target]),
                )?;
                let report = with_commit_failures(Some(commit_failure), recovery_failure, || {
                    prepared.commit()
                });
                assert_eq!(report.outcome.result_state(), expected);
                match expected {
                    CommitResultState::RolledBack => assert!(matches!(
                        report.outcome,
                        MigrationCommitOutcome::RolledBack { .. }
                    )),
                    CommitResultState::RecoveryRequired => assert!(matches!(
                        report.outcome,
                        MigrationCommitOutcome::RecoveryRequired { .. }
                    )),
                    CommitResultState::CommittedCleanupFailed => assert!(matches!(
                        report.outcome,
                        MigrationCommitOutcome::Committed { .. }
                    )),
                    _ => unreachable!(),
                }
                let version = strict_schema_version(&fs::read(root.join("data/a.json"))?)?;
                assert_eq!(version.get(), target_version);
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn marker_post_decision_failure_keeps_migration_core_error_and_recovers_forward() -> TestResult
    {
        with_project(|root, project| {
            let target = target("data/a.json");
            let original = br#"{"schemaVersion":1,"name":"old"}"#.to_vec();
            fs::write(root.join(target.as_str()), &original)?;
            let prepared = prepare_migration(
                MigrationPrepareRequest {
                    targets: vec![target.clone()],
                    registry: registry(),
                },
                project,
                test_write_permit(project.fingerprint(), vec![target.clone()]),
            )?;
            let transaction_id = prepared.transaction_id().clone();
            let directory = prepared.prepared.transaction_directory().to_path_buf();
            let operation = prepared.prepared.manifest().operations[0].clone();
            let staged = fs::read(directory.join(&operation.staged_path))?;

            let report =
                with_commit_failures(Some(CommitTestPoint::CommittedMarkerVerify), None, || {
                    prepared.commit()
                });
            assert_eq!(report.transaction_id, transaction_id);
            assert_eq!(
                report.outcome.result_state(),
                CommitResultState::RecoveryRequired
            );
            let transaction = match &report.outcome {
                MigrationCommitOutcome::RecoveryRequired { transaction } => transaction,
                other => {
                    return Err(format!("unexpected migration marker outcome: {other:?}").into())
                }
            };
            assert!(transaction.rollback_failure().is_none());
            assert_eq!(
                transaction.failure().stage,
                CommitStage::VerifyCommittedMarker
            );
            assert_eq!(transaction.failure().transaction_id, transaction_id);
            assert_eq!(transaction.failure().applied_operations(), &[0]);
            match transaction.failure().source.as_ref() {
                CommitFailureSource::Io(source) => {
                    assert_eq!(source.to_string(), "injected migration transaction failure");
                }
                other => {
                    return Err(format!("unexpected migration marker source: {other:?}").into())
                }
            }

            let marker: CommittedMarker =
                serde_json::from_slice(&fs::read(directory.join("committed.json"))?)?;
            marker.validate()?;
            assert_eq!(marker.transaction_id, transaction_id);
            let state: TransactionStateRecord =
                serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
            assert_eq!(state.state, TransactionState::Applying);
            assert_eq!(state.applied_operations, vec![0]);
            assert_eq!(fs::read(directory.join("backups/000000.json"))?, original);
            assert_eq!(fs::read(root.join(target.as_str()))?, staged);

            let recovered = recover_migration(
                MigrationRecoveryRequest {
                    targets: vec![target.clone()],
                },
                project,
                test_write_permit(project.fingerprint(), vec![target.clone()]),
            )?;
            assert_eq!(recovered.transaction.committed_cleanups, 1);
            assert_eq!(fs::read(root.join(target.as_str()))?, staged);
            assert!(!directory.exists());

            let repeated = recover_migration(
                MigrationRecoveryRequest {
                    targets: vec![target.clone()],
                },
                project,
                test_write_permit(project.fingerprint(), vec![target]),
            )?;
            assert!(repeated.transaction.nothing_to_recover());
            Ok(())
        })
    }

    #[test]
    fn commit_entry_rejects_a_permit_that_becomes_invalid_after_prepare() -> TestResult {
        for category in [
            LockErrorCategory::LockLost,
            LockErrorCategory::LockStateUnknown,
        ] {
            with_project(|root, project| {
                let target = target("data/a.json");
                let original = br#"{"schemaVersion":1,"name":"old"}"#;
                fs::write(root.join(target.as_str()), original)?;
                let (permit, service) =
                    controllable_permit(project.fingerprint(), vec![target.clone()], category)?;
                let prepared = prepare_migration(
                    MigrationPrepareRequest {
                        targets: vec![target.clone()],
                        registry: registry(),
                    },
                    project,
                    permit,
                )?;
                let next_validation = service.validation_calls.load(Ordering::SeqCst) + 1;
                service.fail_at.store(next_validation, Ordering::SeqCst);

                let report = prepared.commit();
                assert_eq!(report.outcome.result_state(), CommitResultState::NotApplied);
                assert!(matches!(
                    report.outcome,
                    MigrationCommitOutcome::CommitFailed { .. }
                ));
                assert_eq!(fs::read(root.join(target.as_str()))?, original);
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn recovery_rejects_lost_and_unknown_permits_without_changing_targets() -> TestResult {
        for category in [
            LockErrorCategory::LockLost,
            LockErrorCategory::LockStateUnknown,
        ] {
            with_project(|root, project| {
                let target = target("data/a.json");
                let original = br#"{"schemaVersion":1,"name":"old"}"#;
                fs::write(root.join(target.as_str()), original)?;
                let prepared = prepare_migration(
                    MigrationPrepareRequest {
                        targets: vec![target.clone()],
                        registry: registry(),
                    },
                    project,
                    test_write_permit(project.fingerprint(), vec![target.clone()]),
                )?;
                let transaction_id = prepared.transaction_id().clone();
                drop(prepared);
                let (permit, service) =
                    controllable_permit(project.fingerprint(), vec![target.clone()], category)?;
                let next_validation = service.validation_calls.load(Ordering::SeqCst) + 1;
                service.fail_at.store(next_validation, Ordering::SeqCst);

                let error = recover_migration(
                    MigrationRecoveryRequest {
                        targets: vec![target.clone()],
                    },
                    project,
                    permit,
                )
                .expect_err("invalid recovery permit must fail");
                assert_eq!(error.stage, MigrationRecoveryStage::PermitValidation);
                assert_eq!(fs::read(root.join(target.as_str()))?, original);
                assert!(project
                    .transactions_root()
                    .join(transaction_id.as_str())
                    .is_dir());
                Ok(())
            })?;
        }
        Ok(())
    }
}
