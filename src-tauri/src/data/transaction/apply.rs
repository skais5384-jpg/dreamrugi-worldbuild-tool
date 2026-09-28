use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

#[cfg(test)]
use super::super::atomic_file::save_deterministic_json_with_after_replace_hook;
use super::super::atomic_file::{save_deterministic_json, SaveError, SaveOutcome};
use super::super::json::to_deterministic_json_bytes;
use super::super::project_file::open_existing_private_file;
use super::super::utc_time::now_utc_milliseconds;
use super::prepare::{managed_schema_version, sha256, sync_directory, ManagedJsonError};
use super::recovery::{
    rollback_manifest, RecoveryError, RecoveryFailPoint, RecoveryHooks, RecoveryResultState,
};
use super::{
    CommittedMarker, LockedProject, PreparedTransaction, ProjectRelativePath, TransactionId,
    TransactionManifest, TransactionModelError, TransactionOperation, TransactionState,
    TransactionStateRecord, WritePermit, WritePermitError,
};
use crate::data::schema::SchemaVersion;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitResultState {
    NotApplied,
    RecoveryRequired,
    Committed,
    CommittedCleanupFailed,
    RolledBack,
    RolledBackCleanupFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommitStage {
    ValidateCommitEntryLock,
    RevalidatePrepared,
    WriteApplyingState,
    CheckTargetPrecondition,
    RevalidateStaged,
    RenameStaged,
    SyncTarget,
    SyncTargetParent,
    VerifyAppliedTarget,
    WriteProgressState,
    ValidateOperationLock,
    ValidateBeforeMarkerLock,
    WriteCommittedMarker,
    VerifyCommittedMarker,
    SyncCommittedMarker,
    WriteCommittedState,
    CleanupTransaction,
}

impl fmt::Display for CommitStage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ValidateCommitEntryLock => "validate collaboration locks at commit entry",
            Self::RevalidatePrepared => "revalidate prepared transaction",
            Self::WriteApplyingState => "write Applying state",
            Self::CheckTargetPrecondition => "check target precondition",
            Self::RevalidateStaged => "revalidate staged artifact",
            Self::RenameStaged => "rename staged artifact to target",
            Self::SyncTarget => "sync applied target",
            Self::SyncTargetParent => "sync target parent directory",
            Self::VerifyAppliedTarget => "verify applied target",
            Self::WriteProgressState => "write applied progress",
            Self::ValidateOperationLock => "validate collaboration lock before target rename",
            Self::ValidateBeforeMarkerLock => "validate collaboration locks before commit marker",
            Self::WriteCommittedMarker => "write committed marker",
            Self::VerifyCommittedMarker => "verify committed marker",
            Self::SyncCommittedMarker => "sync committed marker and transaction directory",
            Self::WriteCommittedState => "write Committed state",
            Self::CleanupTransaction => "clean transaction directory",
        })
    }
}

pub(crate) enum CommitFailureSource {
    Io(io::Error),
    AtomicSave(SaveError),
    Json(serde_json::Error),
    ManagedJson(ManagedJsonError),
    Model(TransactionModelError),
    Permit(WritePermitError),
}

impl fmt::Debug for CommitFailureSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(source) => formatter
                .debug_struct("CommitFailureSource::Io")
                .field("kind", &source.kind())
                // wrapper가 보유한 private 문맥은 숨기고 원 I/O의 OS code만 조회한다.
                .field("os_code", &super::protocol::io_cause(source).raw_os_error())
                .finish(),
            Self::AtomicSave(source) => formatter
                .debug_tuple("CommitFailureSource::AtomicSave")
                .field(source)
                .finish(),
            Self::Json(source) => formatter
                .debug_struct("CommitFailureSource::Json")
                .field("category", &source.classify())
                .field("line", &source.line())
                .field("column", &source.column())
                .finish(),
            Self::ManagedJson(_) => {
                formatter.write_str("CommitFailureSource::ManagedJson([redacted])")
            }
            Self::Model(_) => formatter.write_str("CommitFailureSource::Model([redacted])"),
            Self::Permit(_) => formatter.write_str("CommitFailureSource::Permit([redacted])"),
        }
    }
}

impl fmt::Display for CommitFailureSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("filesystem operation failed"),
            Self::AtomicSave(_) => formatter.write_str("atomic JSON save failed"),
            Self::Json(_) => formatter.write_str("transaction JSON is invalid"),
            Self::ManagedJson(source) => source.fmt(formatter),
            Self::Model(source) => source.fmt(formatter),
            Self::Permit(source) => source.fmt(formatter),
        }
    }
}

impl std::error::Error for CommitFailureSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // 원본 typed source는 각 variant 안에 보존하되, 경로나 본문을 포함할 수 있는
        // 하위 오류를 자동 source chain에 연결하지 않는다.
        None
    }
}

#[derive(Debug)]
pub(crate) struct CommitFailure {
    pub(crate) stage: CommitStage,
    pub(crate) transaction_id: TransactionId,
    pub(crate) project_fingerprint: String,
    pub(crate) target: Option<Box<ProjectRelativePath>>,
    /// 파일 검증까지 끝난 연속 prefix다. recovery는 이 값만 신뢰해서는 안 된다.
    pub(crate) applied_operations: Vec<u32>,
    pub(crate) source: Box<CommitFailureSource>,
}

#[derive(Debug)]
pub(crate) enum TransactionCommitError {
    NotApplied(Box<CommitFailure>),
    RecoveryRequired {
        failure: Box<CommitFailure>,
        rollback_failure: Option<Box<RecoveryError>>,
    },
}

impl TransactionCommitError {
    pub(crate) fn result_state(&self) -> CommitResultState {
        match self {
            Self::NotApplied(_) => CommitResultState::NotApplied,
            Self::RecoveryRequired { .. } => CommitResultState::RecoveryRequired,
        }
    }

    pub(crate) fn failure(&self) -> &CommitFailure {
        match self {
            Self::NotApplied(failure) => failure,
            Self::RecoveryRequired { failure, .. } => failure,
        }
    }

    pub(crate) fn rollback_failure(&self) -> Option<&RecoveryError> {
        match self {
            Self::RecoveryRequired {
                rollback_failure, ..
            } => rollback_failure.as_deref(),
            Self::NotApplied(_) => None,
        }
    }
}

impl CommitFailure {
    pub(crate) fn applied_operations(&self) -> &[u32] {
        &self.applied_operations
    }
}

impl fmt::Display for TransactionCommitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let failure = self.failure();
        let action = match self {
            Self::NotApplied(_) => "no target was changed; fix the cause and prepare again",
            Self::RecoveryRequired { .. } => {
                "commit was not confirmed; preserve transaction data for recovery"
            }
        };
        write!(
            formatter,
            "failed to {} for transaction {} in project {}",
            failure.stage, failure.transaction_id, failure.project_fingerprint
        )?;
        if let Some(target) = &failure.target {
            write!(formatter, ", target '{target}'")?;
        }
        write!(formatter, "; {action}")
    }
}

impl std::error::Error for TransactionCommitError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.failure().source)
    }
}

#[derive(Debug)]
pub(crate) enum CommitOutcome {
    Committed,
    RolledBack {
        apply_failure: Box<CommitFailure>,
    },
    RolledBackCleanupFailed {
        apply_failure: Box<CommitFailure>,
        cleanup_failure: Box<RecoveryError>,
    },
    CommittedCleanupFailed {
        failure: Box<CommitFailure>,
        cleanup_failure: Option<Box<CommitFailure>>,
    },
}

impl CommitOutcome {
    pub(crate) fn result_state(&self) -> CommitResultState {
        match self {
            Self::Committed => CommitResultState::Committed,
            Self::CommittedCleanupFailed { .. } => CommitResultState::CommittedCleanupFailed,
            Self::RolledBack { .. } => CommitResultState::RolledBack,
            Self::RolledBackCleanupFailed { .. } => CommitResultState::RolledBackCleanupFailed,
        }
    }

    pub(crate) fn failures(&self) -> Option<(&CommitFailure, Option<&CommitFailure>)> {
        match self {
            Self::Committed => None,
            Self::CommittedCleanupFailed {
                failure,
                cleanup_failure,
            } => Some((failure, cleanup_failure.as_deref())),
            Self::RolledBack { .. } | Self::RolledBackCleanupFailed { .. } => None,
        }
    }

    pub(crate) fn rollback_failures(&self) -> Option<(&CommitFailure, Option<&RecoveryError>)> {
        match self {
            Self::RolledBack { apply_failure } => Some((apply_failure, None)),
            Self::RolledBackCleanupFailed {
                apply_failure,
                cleanup_failure,
            } => Some((apply_failure, Some(cleanup_failure))),
            Self::Committed | Self::CommittedCleanupFailed { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommitFailPoint {
    ManifestRevalidation,
    ApplyingState,
    TargetPrecondition,
    StagedRevalidation,
    Rename,
    TargetSync,
    TargetParentSync,
    AppliedVerification,
    ProgressState,
    CommittedMarkerWrite,
    #[cfg(test)]
    CommittedMarkerAfterReplace,
    CommittedMarkerVerify,
    CommittedMarkerSync,
    CommittedState,
    Cleanup,
}

pub(super) trait CommitHooks {
    fn check(&self, point: CommitFailPoint, operation: Option<u32>) -> io::Result<()>;

    fn check_recovery(&self, _point: RecoveryFailPoint, _operation: Option<u32>) -> io::Result<()> {
        Ok(())
    }
}

struct NoCommitHooks;

impl CommitHooks for NoCommitHooks {
    fn check(&self, _point: CommitFailPoint, _operation: Option<u32>) -> io::Result<()> {
        Ok(())
    }
}

impl<'project, 'lock, 'guard> PreparedTransaction<'project, 'lock, 'guard> {
    /// 소유권을 소비하므로 같은 prepared transaction을 두 번 commit할 수 없다.
    pub(crate) fn commit(self) -> Result<CommitOutcome, TransactionCommitError> {
        commit_with_hooks(self, &NoCommitHooks)
    }
}

pub(super) fn commit_with_hooks<H>(
    mut prepared: PreparedTransaction<'_, '_, '_>,
    hooks: &H,
) -> Result<CommitOutcome, TransactionCommitError>
where
    H: CommitHooks,
{
    let permit_targets: Vec<_> = prepared
        .manifest
        .operations
        .iter()
        .map(|operation| operation.target_path.clone())
        .collect();
    let context = CommitContext::new(&prepared);
    prepared
        .permit
        .validate_for(context.project.fingerprint(), &permit_targets)
        .map_err(|source| {
            TransactionCommitError::NotApplied(Box::new(context.failure(
                pending(
                    CommitStage::ValidateCommitEntryLock,
                    CommitFailureSource::Permit(source),
                ),
                &[],
            )))
        })?;
    let manifest = revalidate_all(&prepared, hooks).map_err(|failure| {
        TransactionCommitError::NotApplied(Box::new(context.failure(failure, &[])))
    })?;

    write_state(
        &context,
        TransactionState::Applying,
        &[],
        hooks,
        CommitFailPoint::ApplyingState,
        CommitStage::WriteApplyingState,
    )
    .map_err(|failure| TransactionCommitError::NotApplied(Box::new(failure)))?;

    let mut applied = Vec::with_capacity(manifest.operations.len());
    for operation in &manifest.operations {
        if let Err(error) = apply_operation(
            &context,
            &mut prepared.permit,
            &permit_targets,
            operation,
            &mut applied,
            hooks,
        ) {
            return handle_pre_marker_failure(error, &context, &manifest, hooks);
        }
    }

    let marker = match write_and_verify_committed_marker(
        &context,
        &mut prepared.permit,
        &permit_targets,
        &applied,
        hooks,
    ) {
        Ok(marker) => marker,
        Err(MarkerFailure::BeforeCommitDecision(failure)) => {
            return handle_pre_marker_failure(
                TransactionCommitError::RecoveryRequired {
                    failure: Box::new(failure),
                    rollback_failure: None,
                },
                &context,
                &manifest,
                hooks,
            );
        }
        Err(MarkerFailure::AfterCommitDecision(failure)) => {
            // marker가 적용됐거나 적용됐을 가능성이 생긴 뒤에는 recovery만 marker의
            // 실제 존재 여부를 판정한다. 이 경로에서는 marker 삭제나 rollback을 하지 않는다.
            return Err(TransactionCommitError::RecoveryRequired {
                failure: Box::new(failure),
                rollback_failure: None,
            });
        }
    };
    debug_assert_eq!(marker.transaction_id, context.transaction_id);

    let state_failure = write_state(
        &context,
        TransactionState::Committed,
        &applied,
        hooks,
        CommitFailPoint::CommittedState,
        CommitStage::WriteCommittedState,
    )
    .err();

    let cleanup_failure = cleanup_transaction(&context, hooks).err();
    match (state_failure, cleanup_failure) {
        (None, None) => Ok(CommitOutcome::Committed),
        (Some(failure), cleanup_failure) => Ok(CommitOutcome::CommittedCleanupFailed {
            failure: Box::new(failure),
            cleanup_failure: cleanup_failure.map(Box::new),
        }),
        (None, Some(failure)) => Ok(CommitOutcome::CommittedCleanupFailed {
            failure: Box::new(failure),
            cleanup_failure: None,
        }),
    }
}

fn handle_pre_marker_failure<H: CommitHooks>(
    error: TransactionCommitError,
    context: &CommitContext<'_, '_>,
    manifest: &TransactionManifest,
    hooks: &H,
) -> Result<CommitOutcome, TransactionCommitError> {
    let TransactionCommitError::RecoveryRequired { failure, .. } = error else {
        return Err(error);
    };
    match rollback_manifest(
        context.project,
        &context.transaction_id,
        &context.transaction_directory,
        manifest,
        &CommitRecoveryAdapter(hooks),
    ) {
        Ok(()) => Ok(CommitOutcome::RolledBack {
            apply_failure: failure,
        }),
        Err(rollback_failure)
            if rollback_failure.result_state() == RecoveryResultState::RolledBackCleanupFailed =>
        {
            Ok(CommitOutcome::RolledBackCleanupFailed {
                apply_failure: failure,
                cleanup_failure: Box::new(rollback_failure),
            })
        }
        Err(rollback_failure) => Err(TransactionCommitError::RecoveryRequired {
            failure,
            rollback_failure: Some(Box::new(rollback_failure)),
        }),
    }
}

struct CommitRecoveryAdapter<'a, H>(&'a H);

impl<H: CommitHooks> RecoveryHooks for CommitRecoveryAdapter<'_, H> {
    fn check(&self, point: RecoveryFailPoint, operation: Option<u32>) -> io::Result<()> {
        self.0.check_recovery(point, operation)
    }
}

struct CommitContext<'project, 'lock> {
    project: &'project LockedProject<'lock>,
    transaction_id: TransactionId,
    transaction_directory: PathBuf,
    schema_version: SchemaVersion,
}

impl<'project, 'lock> CommitContext<'project, 'lock> {
    fn new(prepared: &PreparedTransaction<'project, 'lock, '_>) -> Self {
        Self {
            project: prepared.project,
            schema_version: prepared.manifest.schema_version,
            transaction_id: prepared.transaction_id.clone(),
            transaction_directory: prepared.transaction_directory.clone(),
        }
    }

    fn failure(&self, failure: PendingFailure, applied_operations: &[u32]) -> CommitFailure {
        CommitFailure {
            stage: failure.stage,
            transaction_id: self.transaction_id.clone(),
            project_fingerprint: self.project.fingerprint().to_owned(),
            target: failure.target.map(Box::new),
            applied_operations: applied_operations.to_vec(),
            source: failure.source,
        }
    }
}

struct PendingFailure {
    stage: CommitStage,
    target: Option<ProjectRelativePath>,
    source: Box<CommitFailureSource>,
}

fn pending(stage: CommitStage, source: impl Into<CommitFailureSource>) -> PendingFailure {
    PendingFailure {
        stage,
        target: None,
        source: Box::new(source.into()),
    }
}

impl From<io::Error> for CommitFailureSource {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

fn with_target(mut failure: PendingFailure, target: &ProjectRelativePath) -> PendingFailure {
    failure.target = Some(target.clone());
    failure
}

fn revalidate_all<H>(
    prepared: &PreparedTransaction<'_, '_, '_>,
    hooks: &H,
) -> Result<TransactionManifest, PendingFailure>
where
    H: CommitHooks,
{
    hooks
        .check(CommitFailPoint::ManifestRevalidation, None)
        .map_err(|source| pending(CommitStage::RevalidatePrepared, source))?;
    let directory = &prepared.transaction_directory;
    let journal = super::protocol::inspect(directory)
        .map_err(|e| pending(CommitStage::RevalidatePrepared, e))?;
    journal
        .validate_project(prepared.project)
        .map_err(|e| pending(CommitStage::RevalidatePrepared, e))?;
    if journal
        .version()
        .map_err(|e| pending(CommitStage::RevalidatePrepared, e))?
        != prepared.manifest.schema_version
    {
        return Err(invalid(
            "prepared protocol differs from mandatory journal records",
        ));
    }
    ensure_absent(&directory.join("committed.json"))?;
    ensure_absent(&directory.join("rolled-back.json"))?;

    let state: TransactionStateRecord = read_json(&directory.join("state.json"))?;
    state.validate().map_err(|source| {
        pending(
            CommitStage::RevalidatePrepared,
            CommitFailureSource::Model(source),
        )
    })?;
    if state.state != TransactionState::Prepared
        || !state.applied_operations.is_empty()
        || state.transaction_id != prepared.transaction_id
        || state.project_fingerprint != prepared.project.fingerprint()
    {
        return Err(invalid("state.json is not the expected Prepared record"));
    }

    let manifest: TransactionManifest = read_json(&directory.join("manifest.json"))?;
    manifest.validate().map_err(|source| {
        pending(
            CommitStage::RevalidatePrepared,
            CommitFailureSource::Model(source),
        )
    })?;
    if manifest != prepared.manifest
        || manifest.transaction_id != prepared.transaction_id
        || manifest.project_fingerprint != prepared.project.fingerprint()
    {
        return Err(invalid("immutable manifest identity or contents changed"));
    }

    for operation in &manifest.operations {
        resolve_target(prepared.project, &operation.target_path)
            .map_err(|failure| with_target(failure, &operation.target_path))?;
        verify_artifact(
            &directory.join(&operation.staged_path),
            operation.staged_size,
            &operation.staged_sha256,
            operation.staged_schema_version,
        )
        .map_err(|failure| with_target(failure, &operation.target_path))?;
        if operation.original_existed {
            let backup_path = operation
                .backup_path
                .as_deref()
                .ok_or_else(|| invalid("existing target backup path is missing"))?;
            verify_artifact(
                &directory.join(backup_path),
                operation
                    .original_size
                    .ok_or_else(|| invalid("original size is missing"))?,
                operation
                    .original_sha256
                    .as_deref()
                    .ok_or_else(|| invalid("original hash is missing"))?,
                operation.original_schema_version,
            )
            .map_err(|failure| with_target(failure, &operation.target_path))?;
        }
    }
    Ok(manifest)
}

fn apply_operation<H>(
    context: &CommitContext<'_, '_>,
    permit: &mut WritePermit<'_>,
    permit_targets: &[ProjectRelativePath],
    operation: &TransactionOperation,
    applied: &mut Vec<u32>,
    hooks: &H,
) -> Result<(), TransactionCommitError>
where
    H: CommitHooks,
{
    let target = resolve_target(context.project, &operation.target_path).map_err(|failure| {
        apply_error(
            context,
            with_target(failure, &operation.target_path),
            applied,
        )
    })?;
    hooks
        .check(CommitFailPoint::TargetPrecondition, Some(operation.index))
        .and_then(|()| check_precondition(&target, operation))
        .map_err(|source| {
            apply_error(
                context,
                with_target(
                    pending(CommitStage::CheckTargetPrecondition, source),
                    &operation.target_path,
                ),
                applied,
            )
        })?;

    let staged = context.transaction_directory.join(&operation.staged_path);
    hooks
        .check(CommitFailPoint::StagedRevalidation, Some(operation.index))
        .map_err(|source| pending(CommitStage::RevalidateStaged, source))
        .and_then(|()| {
            verify_artifact(
                &staged,
                operation.staged_size,
                &operation.staged_sha256,
                operation.staged_schema_version,
            )
        })
        .map_err(|mut failure| {
            failure.stage = CommitStage::RevalidateStaged;
            apply_error(
                context,
                with_target(failure, &operation.target_path),
                applied,
            )
        })?;

    hooks
        .check(CommitFailPoint::Rename, Some(operation.index))
        .map_err(|source| {
            recovery_error(
                context,
                operation,
                CommitStage::RenameStaged,
                source,
                applied,
            )
        })?;

    let mut owned =
        if super::owned_temp::enabled(&context.transaction_directory).map_err(|source| {
            recovery_error(
                context,
                operation,
                CommitStage::RenameStaged,
                source,
                applied,
            )
        })? {
            Some(
                super::owned_temp::Replacement::apply(
                    context.project,
                    &context.transaction_id,
                    &context.transaction_directory,
                    operation,
                )
                .map_err(|source| {
                    recovery_error(
                        context,
                        operation,
                        CommitStage::RenameStaged,
                        source,
                        applied,
                    )
                })?,
            )
        } else {
            None
        };
    if owned.is_some() {
        check_precondition(&target, operation).map_err(|source| {
            recovery_error(
                context,
                operation,
                CommitStage::CheckTargetPrecondition,
                source,
                applied,
            )
        })?;
    }
    if let Err(source) = permit.validate_for(context.project.fingerprint(), permit_targets) {
        return Err(apply_error(
            context,
            with_target(
                pending(
                    CommitStage::ValidateOperationLock,
                    CommitFailureSource::Permit(source),
                ),
                &operation.target_path,
            ),
            applied,
        ));
    }

    // 최종 source/permit 검사 뒤에는 소유 handle의 parent 검사와 원자 rename만 수행한다.
    // 호출자 callback이나 파일 경로 재개방을 사이에 넣지 않는다.
    match &mut owned {
        Some(file) => file.rename(operation.original_existed),
        None => fs::rename(&staged, &target),
    }
    .map_err(|source| {
        recovery_error(
            context,
            operation,
            CommitStage::RenameStaged,
            source,
            applied,
        )
    })?;
    hooks
        .check(CommitFailPoint::TargetSync, Some(operation.index))
        .and_then(|()| match &owned {
            Some(file) => file.sync(),
            None => OpenOptions::new()
                .read(true)
                .write(true)
                .open(&target)?
                .sync_all(),
        })
        .map_err(|source| {
            recovery_error(context, operation, CommitStage::SyncTarget, source, applied)
        })?;
    hooks
        .check(CommitFailPoint::TargetParentSync, Some(operation.index))
        .and_then(|()| {
            sync_directory(
                target
                    .parent()
                    .ok_or_else(|| io::Error::other("target parent missing"))?,
            )
        })
        .map_err(|source| {
            recovery_error(
                context,
                operation,
                CommitStage::SyncTargetParent,
                source,
                applied,
            )
        })?;
    hooks
        .check(CommitFailPoint::AppliedVerification, Some(operation.index))
        .map_err(|source| pending(CommitStage::VerifyAppliedTarget, source))
        .and_then(|()| {
            if let Some(file) = &mut owned {
                file.verify()
                    .map_err(|source| pending(CommitStage::VerifyAppliedTarget, source))
            } else {
                verify_artifact(
                    &target,
                    operation.staged_size,
                    &operation.staged_sha256,
                    operation.staged_schema_version,
                )
            }
        })
        .map_err(|mut failure| {
            failure.stage = CommitStage::VerifyAppliedTarget;
            TransactionCommitError::RecoveryRequired {
                failure: Box::new(
                    context.failure(with_target(failure, &operation.target_path), applied),
                ),
                rollback_failure: None,
            }
        })?;

    drop(owned);
    super::owned_temp::checkpoint("before-progress", operation.index).map_err(|source| {
        recovery_error(
            context,
            operation,
            CommitStage::WriteProgressState,
            source,
            applied,
        )
    })?;
    applied.push(operation.index);
    write_state(
        context,
        TransactionState::Applying,
        applied,
        hooks,
        CommitFailPoint::ProgressState,
        CommitStage::WriteProgressState,
    )
    .map_err(|failure| TransactionCommitError::RecoveryRequired {
        failure: Box::new(failure),
        rollback_failure: None,
    })
}

fn apply_error(
    context: &CommitContext<'_, '_>,
    failure: PendingFailure,
    applied: &[u32],
) -> TransactionCommitError {
    let failure = context.failure(failure, applied);
    if applied.is_empty() {
        TransactionCommitError::NotApplied(Box::new(failure))
    } else {
        TransactionCommitError::RecoveryRequired {
            failure: Box::new(failure),
            rollback_failure: None,
        }
    }
}

fn recovery_error(
    context: &CommitContext<'_, '_>,
    operation: &TransactionOperation,
    stage: CommitStage,
    source: io::Error,
    applied: &[u32],
) -> TransactionCommitError {
    TransactionCommitError::RecoveryRequired {
        failure: Box::new(context.failure(
            with_target(pending(stage, source), &operation.target_path),
            applied,
        )),
        rollback_failure: None,
    }
}

fn write_and_verify_committed_marker<H>(
    context: &CommitContext<'_, '_>,
    permit: &mut WritePermit<'_>,
    permit_targets: &[ProjectRelativePath],
    applied: &[u32],
    hooks: &H,
) -> Result<CommittedMarker, MarkerFailure>
where
    H: CommitHooks,
{
    let completed_at_utc = now_utc_milliseconds().map_err(|source| {
        MarkerFailure::BeforeCommitDecision(context.failure(
            pending(
                CommitStage::WriteCommittedMarker,
                CommitFailureSource::Model(TransactionModelError::TransactionTimeFailed { source }),
            ),
            applied,
        ))
    })?;
    let marker = CommittedMarker {
        schema_version: context.schema_version,
        transaction_id: context.transaction_id.clone(),
        completed_at_utc,
        project_fingerprint: context.project.fingerprint().to_owned(),
    };
    marker.validate().map_err(|source| {
        MarkerFailure::BeforeCommitDecision(context.failure(
            pending(
                CommitStage::WriteCommittedMarker,
                CommitFailureSource::Model(source),
            ),
            applied,
        ))
    })?;
    let path = context.transaction_directory.join("committed.json");
    hooks
        .check(CommitFailPoint::CommittedMarkerWrite, None)
        .map_err(|source| {
            MarkerFailure::BeforeCommitDecision(
                context.failure(pending(CommitStage::WriteCommittedMarker, source), applied),
            )
        })?;
    permit
        .validate_for(context.project.fingerprint(), permit_targets)
        .map_err(|source| {
            MarkerFailure::BeforeCommitDecision(context.failure(
                pending(
                    CommitStage::ValidateBeforeMarkerLock,
                    CommitFailureSource::Permit(source),
                ),
                applied,
            ))
        })?;
    // marker용 값과 failpoint를 모두 준비한 뒤 잠금을 검증하고 즉시 원자 저장한다.
    #[cfg(test)]
    let save_result = save_deterministic_json_with_after_replace_hook(&path, &marker, |_| {
        hooks.check(CommitFailPoint::CommittedMarkerAfterReplace, None)
    });
    #[cfg(not(test))]
    let save_result = save_deterministic_json(&path, &marker);
    save_result.map_err(|source| {
        let outcome = match &source {
            SaveError::Serialize(_) => SaveOutcome::NotApplied,
            SaveError::AtomicWrite(error) => error.outcome,
        };
        let failure = context.failure(
            pending(
                CommitStage::WriteCommittedMarker,
                CommitFailureSource::AtomicSave(source),
            ),
            applied,
        );
        match outcome {
            SaveOutcome::NotApplied => MarkerFailure::BeforeCommitDecision(failure),
            SaveOutcome::AppliedDurabilityUncertain => MarkerFailure::AfterCommitDecision(failure),
        }
    })?;
    hooks
        .check(CommitFailPoint::CommittedMarkerVerify, None)
        .map_err(|source| {
            MarkerFailure::AfterCommitDecision(
                context.failure(pending(CommitStage::VerifyCommittedMarker, source), applied),
            )
        })?;
    let mut marker_file = open_existing_private_file(&context.transaction_directory, &path)
        .map_err(|source| {
            MarkerFailure::AfterCommitDecision(
                context.failure(pending(CommitStage::VerifyCommittedMarker, source), applied),
            )
        })?;
    let mut marker_bytes = Vec::new();
    marker_file
        .read_to_end(&mut marker_bytes)
        .map_err(|source| {
            MarkerFailure::AfterCommitDecision(
                context.failure(pending(CommitStage::VerifyCommittedMarker, source), applied),
            )
        })?;
    // Windows의 no-follow read handle은 쓰기/교체를 막으므로 후속 명시적 sync 전에 닫는다.
    drop(marker_file);
    let decoded: CommittedMarker = serde_json::from_slice(&marker_bytes).map_err(|source| {
        MarkerFailure::AfterCommitDecision(context.failure(
            pending(
                CommitStage::VerifyCommittedMarker,
                CommitFailureSource::Json(source),
            ),
            applied,
        ))
    })?;
    decoded.validate().map_err(|source| {
        MarkerFailure::AfterCommitDecision(context.failure(
            pending(
                CommitStage::VerifyCommittedMarker,
                CommitFailureSource::Model(source),
            ),
            applied,
        ))
    })?;
    let expected_bytes = to_deterministic_json_bytes(&marker).map_err(|source| {
        MarkerFailure::AfterCommitDecision(context.failure(
            pending(
                CommitStage::VerifyCommittedMarker,
                CommitFailureSource::Json(source),
            ),
            applied,
        ))
    })?;
    if decoded != marker || marker_bytes != expected_bytes {
        return Err(MarkerFailure::AfterCommitDecision(context.failure(
            invalid_at(
                CommitStage::VerifyCommittedMarker,
                "committed marker differs",
            ),
            applied,
        )));
    }
    hooks
        .check(CommitFailPoint::CommittedMarkerSync, None)
        .and_then(|()| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(&path)?
                .sync_all()
        })
        .and_then(|()| sync_directory(&context.transaction_directory))
        .map_err(|source| {
            MarkerFailure::AfterCommitDecision(
                context.failure(pending(CommitStage::SyncCommittedMarker, source), applied),
            )
        })?;
    Ok(marker)
}

enum MarkerFailure {
    /// committed marker atomic save가 적용되지 않았다고 확정된 실패만 포함한다.
    BeforeCommitDecision(CommitFailure),
    /// marker가 적용됐거나 적용 가능성이 생긴 뒤의 모든 실패를 포함한다.
    AfterCommitDecision(CommitFailure),
}

fn write_state<H>(
    context: &CommitContext<'_, '_>,
    state: TransactionState,
    applied: &[u32],
    hooks: &H,
    point: CommitFailPoint,
    stage: CommitStage,
) -> Result<(), CommitFailure>
where
    H: CommitHooks,
{
    let updated_at_utc = now_utc_milliseconds().map_err(|source| {
        context.failure(
            pending(
                stage,
                CommitFailureSource::Model(TransactionModelError::TransactionTimeFailed { source }),
            ),
            applied,
        )
    })?;
    let record = TransactionStateRecord {
        schema_version: context.schema_version,
        transaction_id: context.transaction_id.clone(),
        updated_at_utc,
        project_fingerprint: context.project.fingerprint().to_owned(),
        state,
        applied_operations: applied.to_vec(),
        original_targets: super::protocol::inspect(&context.transaction_directory)
            .map_err(|source| context.failure(pending(stage, source), applied))?
            .state
            .and_then(|state| state.original_targets),
    };
    record.validate().map_err(|source| {
        context.failure(pending(stage, CommitFailureSource::Model(source)), applied)
    })?;
    hooks
        .check(point, applied.last().copied())
        .map_err(|source| context.failure(pending(stage, source), applied))?;
    save_deterministic_json(&context.transaction_directory.join("state.json"), &record).map_err(
        |source| {
            context.failure(
                pending(stage, CommitFailureSource::AtomicSave(source)),
                applied,
            )
        },
    )
}

fn cleanup_transaction<H>(context: &CommitContext<'_, '_>, hooks: &H) -> Result<(), CommitFailure>
where
    H: CommitHooks,
{
    hooks
        .check(CommitFailPoint::Cleanup, None)
        .and_then(|()| super::owned_temp::checkpoint("committed-cleanup", 0))
        .and_then(|()| {
            super::recovery::cleanup_committed(
                context.project,
                &context.transaction_id,
                &context.transaction_directory,
            )
        })
        .map_err(|source| context.failure(pending(CommitStage::CleanupTransaction, source), &[]))
}

fn read_json<T>(path: &Path) -> Result<T, PendingFailure>
where
    T: serde::de::DeserializeOwned,
{
    read_json_at(path, CommitStage::RevalidatePrepared)
}

fn read_json_at<T>(path: &Path, stage: CommitStage) -> Result<T, PendingFailure>
where
    T: serde::de::DeserializeOwned,
{
    let bytes = fs::read(path).map_err(|source| pending(stage, source))?;
    serde_json::from_slice(&bytes)
        .map_err(|source| pending(stage, CommitFailureSource::Json(source)))
}

fn ensure_absent(path: &Path) -> Result<(), PendingFailure> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(invalid("completion marker already exists")),
        Err(source) => Err(pending(CommitStage::RevalidatePrepared, source)),
    }
}

fn verify_artifact(
    path: &Path,
    expected_size: u64,
    expected_hash: &str,
    expected_schema_version: Option<SchemaVersion>,
) -> Result<(), PendingFailure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|source| pending(CommitStage::RevalidatePrepared, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(invalid("artifact is not a regular file"));
    }
    let bytes =
        fs::read(path).map_err(|source| pending(CommitStage::RevalidatePrepared, source))?;
    if bytes.len() as u64 != expected_size || sha256(&bytes) != expected_hash {
        return Err(invalid("artifact size or SHA-256 differs from manifest"));
    }
    let schema_version = managed_schema_version(&bytes).map_err(|source| {
        pending(
            CommitStage::RevalidatePrepared,
            CommitFailureSource::ManagedJson(source),
        )
    })?;
    if expected_schema_version.is_some_and(|expected| schema_version != expected) {
        return Err(invalid("artifact schemaVersion differs from manifest"));
    }
    Ok(())
}

fn resolve_target(
    project: &LockedProject<'_>,
    target: &ProjectRelativePath,
) -> Result<PathBuf, PendingFailure> {
    ProjectRelativePath::parse(target.as_str()).map_err(|source| {
        pending(
            CommitStage::RevalidatePrepared,
            CommitFailureSource::Model(source.into()),
        )
    })?;
    let absolute = project.canonical_root().join(target.as_str());
    let parent = absolute
        .parent()
        .ok_or_else(|| invalid("target parent is missing"))?;
    let metadata = fs::symlink_metadata(parent)
        .map_err(|source| pending(CommitStage::RevalidatePrepared, source))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid("target parent is not a real directory"));
    }
    let canonical = fs::canonicalize(parent)
        .map_err(|source| pending(CommitStage::RevalidatePrepared, source))?;
    if !canonical.starts_with(project.canonical_root()) {
        return Err(invalid("target parent resolves outside the project"));
    }
    Ok(absolute)
}

fn check_precondition(path: &Path, operation: &TransactionOperation) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(io::Error::other("target is not the expected regular file"))
        }
        Ok(_) if !operation.original_existed => Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "new target now exists",
        )),
        Ok(_) => {
            let bytes = fs::read(path)?;
            if Some(bytes.len() as u64) != operation.original_size
                || operation.original_sha256.as_deref() != Some(sha256(&bytes).as_str())
            {
                return Err(io::Error::other(
                    "existing target differs from prepared original",
                ));
            }
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && !operation.original_existed => {
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(io::Error::new(
            io::ErrorKind::NotFound,
            "existing target disappeared",
        )),
        Err(error) => Err(error),
    }
}

fn invalid(reason: &'static str) -> PendingFailure {
    invalid_at(CommitStage::RevalidatePrepared, reason)
}

fn invalid_at(stage: CommitStage, reason: &'static str) -> PendingFailure {
    pending(stage, io::Error::new(io::ErrorKind::InvalidData, reason))
}

// 실제 재검증 오류의 소유권 전달 경로만 시험한다. 디스크 장애를 가장하지 않는다.
#[cfg(test)]
pub(super) fn fix005_wrap(
    prepared: &PreparedTransaction<'_, '_, '_>,
    error: io::Error,
) -> TransactionCommitError {
    TransactionCommitError::NotApplied(Box::new(
        CommitContext::new(prepared).failure(pending(CommitStage::RevalidatePrepared, error), &[]),
    ))
}
