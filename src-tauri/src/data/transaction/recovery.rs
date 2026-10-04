use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, Metadata, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use super::protocol::MainRecord;

use super::super::atomic_file::{save_bytes_atomic, save_deterministic_json};
use super::super::project_file::open_existing_private_file;
use super::super::utc_time::now_utc_milliseconds;
use super::prepare::{managed_schema_version, sha256, sync_directory};
use super::{
    CommittedMarker, LockedProject, ProjectRelativePath, RolledBackMarker, TransactionId,
    TransactionManifest, TransactionOperation, TransactionState, TransactionStateRecord,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryResultState {
    RolledBackCleanupFailed,
    ManualRecoveryRequired,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RecoveryStage {
    EnumerateTransactions,
    InspectJournal,
    VerifyCommittedTarget,
    VerifyRolledBackTarget,
    WriteRollingBackState,
    VerifyBackup,
    RestoreExistingTarget,
    RemoveNewTarget,
    VerifyRestoredTarget,
    WriteRollbackProgress,
    WriteRolledBackMarker,
    VerifyRolledBackMarker,
    SyncRolledBackMarker,
    WriteRolledBackState,
    CleanupTransaction,
}

#[derive(Debug)]
pub(crate) struct RecoveryFailure {
    pub(crate) stage: RecoveryStage,
    pub(crate) transaction_id: Option<TransactionId>,
    pub(crate) project_fingerprint: String,
    pub(crate) target: Option<Box<ProjectRelativePath>>,
    pub(crate) source: io::Error,
}

impl RecoveryFailure {
    /// 공개 error chain을 열지 않고 주 기록 wrapper 안의 원 OS/custom I/O를 빌린다.
    pub(crate) fn io_cause(&self) -> &io::Error {
        super::protocol::io_cause(&self.source)
    }
}

#[derive(Debug)]
pub(crate) enum RecoveryError {
    RecoveryRequired(Box<RecoveryFailure>),
    ManualRecoveryRequired(Box<RecoveryFailure>),
    RolledBackCleanupFailed(Box<RecoveryFailure>),
}

impl RecoveryError {
    pub(crate) fn result_state(&self) -> RecoveryResultState {
        match self {
            Self::RecoveryRequired(_) => RecoveryResultState::RecoveryRequired,
            Self::ManualRecoveryRequired(_) => RecoveryResultState::ManualRecoveryRequired,
            Self::RolledBackCleanupFailed(_) => RecoveryResultState::RolledBackCleanupFailed,
        }
    }

    pub(crate) fn failure(&self) -> &RecoveryFailure {
        match self {
            Self::RecoveryRequired(value)
            | Self::ManualRecoveryRequired(value)
            | Self::RolledBackCleanupFailed(value) => value,
        }
    }
}

impl fmt::Display for RecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let failure = self.failure();
        write!(
            formatter,
            "transaction recovery failed at {:?}",
            failure.stage
        )?;
        if let Some(id) = &failure.transaction_id {
            write!(formatter, " for {id}")?;
        }
        write!(formatter, " in project {}", failure.project_fingerprint)?;
        if let Some(target) = &failure.target {
            write!(formatter, ", target '{target}'")?;
        }
        match self {
            Self::ManualRecoveryRequired(_) => formatter
                .write_str("; block normal writes and preserve diagnostics for manual recovery"),
            Self::RecoveryRequired(_) => {
                formatter.write_str("; preserve transaction data and retry recovery")
            }
            Self::RolledBackCleanupFailed(_) => formatter
                .write_str("; project data is rolled back but transaction cleanup must be retried"),
        }
    }
}

impl std::error::Error for RecoveryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.failure().source)
    }
}

#[derive(Debug, Default)]
pub(crate) struct RecoveryReport {
    pub(crate) discovered_transactions: usize,
    pub(crate) rolled_back_transactions: usize,
    pub(crate) committed_cleanups: usize,
    pub(crate) rolled_back_cleanups: usize,
    pub(crate) preparing_orphan_cleanups: usize,
    pub(crate) manual_recovery_required: bool,
    pub(crate) transaction_ids: Vec<TransactionId>,
}

impl RecoveryReport {
    pub(crate) fn nothing_to_recover(&self) -> bool {
        self.discovered_transactions == 0
    }

    pub(crate) fn manual_recovery_required(&self) -> bool {
        self.manual_recovery_required
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RecoveryFailPoint {
    Enumerate,
    RollingBackState,
    BackupVerify,
    RestoreExisting,
    RemoveNew,
    RestoredVerify,
    ProgressState,
    RolledBackMarkerWrite,
    RolledBackMarkerVerify,
    RolledBackMarkerSync,
    RolledBackState,
    Cleanup,
    CommittedTargetVerify,
    JournalDecision,
}

pub(super) trait RecoveryHooks {
    fn check(&self, point: RecoveryFailPoint, operation: Option<u32>) -> io::Result<()>;
}

pub(super) struct NoRecoveryHooks;

impl RecoveryHooks for NoRecoveryHooks {
    fn check(&self, _point: RecoveryFailPoint, _operation: Option<u32>) -> io::Result<()> {
        Ok(())
    }
}

/// 기존 recovery 결정을 바꾸지 않으면서, 특정 상위 작업이 허용한 journal만
/// mutation 단계로 진입하게 한다. 검증은 전체 preflight와 실제 처리 직전에 반복된다.
pub(crate) trait RecoveryScope {
    fn validate_manifest(
        &self,
        transaction_id: &TransactionId,
        directory: &Path,
        manifest: &TransactionManifest,
    ) -> Result<(), &'static str>;

    fn validate_missing_manifest(
        &self,
        _transaction_id: &TransactionId,
        _directory: &Path,
    ) -> Result<(), &'static str> {
        Ok(())
    }
}

struct UnrestrictedRecoveryScope;

impl RecoveryScope for UnrestrictedRecoveryScope {
    fn validate_manifest(
        &self,
        _transaction_id: &TransactionId,
        _directory: &Path,
        _manifest: &TransactionManifest,
    ) -> Result<(), &'static str> {
        Ok(())
    }
}

enum RecoveredKind {
    RolledBack,
    CommittedCleanup,
    RolledBackCleanup,
    PreparingCleanup,
}

struct VerifiedTransactionNamespace {
    root: PathBuf,
}

pub(crate) fn recover_pending_transactions(
    project: &LockedProject<'_>,
) -> Result<RecoveryReport, RecoveryError> {
    recover_pending_transactions_with_hooks_and_scope(
        project,
        &NoRecoveryHooks,
        &UnrestrictedRecoveryScope,
    )
}

pub(super) fn recover_pending_transactions_with_hooks<H: RecoveryHooks>(
    project: &LockedProject<'_>,
    hooks: &H,
) -> Result<RecoveryReport, RecoveryError> {
    recover_pending_transactions_with_hooks_and_scope(project, hooks, &UnrestrictedRecoveryScope)
}

#[cfg(not(test))]
pub(crate) fn recover_pending_transactions_with_scope<S: RecoveryScope>(
    project: &LockedProject<'_>,
    scope: &S,
) -> Result<RecoveryReport, RecoveryError> {
    recover_pending_transactions_with_hooks_and_scope(project, &NoRecoveryHooks, scope)
}

pub(super) fn recover_pending_transactions_with_hooks_and_scope<
    H: RecoveryHooks,
    S: RecoveryScope,
>(
    project: &LockedProject<'_>,
    hooks: &H,
    scope: &S,
) -> Result<RecoveryReport, RecoveryError> {
    hooks
        .check(RecoveryFailPoint::Enumerate, None)
        .map_err(|source| {
            required(
                project,
                None,
                None,
                RecoveryStage::EnumerateTransactions,
                source,
            )
        })?;
    let Some(namespace) = verify_transaction_namespace(project)? else {
        return Ok(RecoveryReport::default());
    };
    let entries = fs::read_dir(&namespace.root).map_err(|source| {
        required(
            project,
            None,
            None,
            RecoveryStage::EnumerateTransactions,
            source,
        )
    })?;

    let mut transactions = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|source| {
            required(
                project,
                None,
                None,
                RecoveryStage::EnumerateTransactions,
                source,
            )
        })?;
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            manual(
                project,
                None,
                None,
                RecoveryStage::InspectJournal,
                "non-UTF-8 transaction entry",
            )
        })?;
        let id = TransactionId::parse(name).map_err(|_| {
            manual(
                project,
                None,
                None,
                RecoveryStage::InspectJournal,
                "invalid transaction directory name",
            )
        })?;
        let directory = verify_transaction_directory(project, &id, &namespace.root, &entry.path())?;
        transactions.insert(id, directory);
    }

    preflight_and_reject_overlapping_targets(project, &transactions, scope)?;
    let mut report = RecoveryReport {
        discovered_transactions: transactions.len(),
        ..RecoveryReport::default()
    };
    for (id, directory) in transactions {
        hooks
            .check(RecoveryFailPoint::JournalDecision, None)
            .map_err(|source| {
                required(
                    project,
                    Some(&id),
                    None,
                    RecoveryStage::InspectJournal,
                    source,
                )
            })?;
        let kind = recover_one(project, &id, &directory, hooks, scope)?;
        report.transaction_ids.push(id);
        match kind {
            RecoveredKind::RolledBack => report.rolled_back_transactions += 1,
            RecoveredKind::CommittedCleanup => report.committed_cleanups += 1,
            RecoveredKind::RolledBackCleanup => report.rolled_back_cleanups += 1,
            RecoveredKind::PreparingCleanup => report.preparing_orphan_cleanups += 1,
        }
    }
    Ok(report)
}

/// recovery namespace의 각 component를 따로 검사해 부모 symlink/junction을
/// `read_dir`가 따라가기 전에 차단한다. 정상적인 미생성 상태만 빈 report로 취급한다.
fn verify_transaction_namespace(
    project: &LockedProject<'_>,
) -> Result<Option<VerifiedTransactionNamespace>, RecoveryError> {
    let project_root = verify_real_directory(project.canonical_root(), None).map_err(|source| {
        namespace_error(project, None, RecoveryStage::EnumerateTransactions, source)
    })?;
    let Some(project_root) = project_root else {
        return Err(required(
            project,
            None,
            None,
            RecoveryStage::EnumerateTransactions,
            io::Error::new(io::ErrorKind::NotFound, "canonical project root is missing"),
        ));
    };
    if project_root != project.canonical_root() {
        return Err(manual(
            project,
            None,
            None,
            RecoveryStage::EnumerateTransactions,
            "project root identity changed",
        ));
    }

    let worldbuild = project_root.join(".worldbuild");
    let Some(worldbuild) =
        verify_real_directory(&worldbuild, Some(&project_root)).map_err(|source| {
            namespace_error(project, None, RecoveryStage::EnumerateTransactions, source)
        })?
    else {
        return Ok(None);
    };

    let transactions = worldbuild.join("transactions");
    let Some(root) = verify_real_directory(&transactions, Some(&worldbuild)).map_err(|source| {
        namespace_error(project, None, RecoveryStage::EnumerateTransactions, source)
    })?
    else {
        return Ok(None);
    };
    if !root.starts_with(&project_root) {
        return Err(manual(
            project,
            None,
            None,
            RecoveryStage::EnumerateTransactions,
            "transaction namespace resolves outside the project",
        ));
    }
    Ok(Some(VerifiedTransactionNamespace { root }))
}

fn verify_transaction_directory(
    project: &LockedProject<'_>,
    id: &TransactionId,
    root: &Path,
    directory: &Path,
) -> Result<PathBuf, RecoveryError> {
    let verified = verify_real_directory(directory, Some(root))
        .map_err(|source| {
            namespace_error(project, Some(id), RecoveryStage::InspectJournal, source)
        })?
        .ok_or_else(|| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                io::Error::new(io::ErrorKind::NotFound, "transaction directory disappeared"),
            )
        })?;
    if verified.file_name() != Some(id.as_str().as_ref()) {
        return Err(manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "transaction directory identity changed",
        ));
    }
    Ok(verified)
}

fn verify_real_directory(
    path: &Path,
    expected_parent: Option<&Path>,
) -> io::Result<Option<PathBuf>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !is_real_directory(&metadata) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "transaction namespace component is not a regular directory",
        ));
    }
    let canonical = fs::canonicalize(path)?;
    if expected_parent.is_some_and(|parent| canonical.parent() != Some(parent)) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "transaction namespace component changed parent",
        ));
    }
    Ok(Some(canonical))
}

fn is_real_directory(metadata: &Metadata) -> bool {
    metadata.is_dir() && !metadata.file_type().is_symlink() && !is_windows_reparse(metadata)
}

fn is_real_file(metadata: &Metadata) -> bool {
    metadata.is_file() && !metadata.file_type().is_symlink() && !is_windows_reparse(metadata)
}

#[cfg(windows)]
fn is_windows_reparse(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn is_windows_reparse(_metadata: &Metadata) -> bool {
    false
}

fn namespace_error(
    project: &LockedProject<'_>,
    id: Option<&TransactionId>,
    stage: RecoveryStage,
    source: io::Error,
) -> RecoveryError {
    if matches!(
        source.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::InvalidData | io::ErrorKind::NotADirectory
    ) {
        manual(
            project,
            id,
            None,
            stage,
            "transaction namespace cannot be proven safe",
        )
    } else {
        required(project, id, None, stage, source)
    }
}

fn protocol_error(
    project: &LockedProject<'_>,
    id: &TransactionId,
    source: io::Error,
) -> RecoveryError {
    let manual = matches!(
        source.kind(),
        io::ErrorKind::InvalidData | io::ErrorKind::PermissionDenied | io::ErrorKind::NotADirectory
    );
    let failure = Box::new(RecoveryFailure {
        stage: RecoveryStage::InspectJournal,
        transaction_id: Some(id.clone()),
        project_fingerprint: project.fingerprint().to_owned(),
        target: None,
        source,
    });
    if manual {
        RecoveryError::ManualRecoveryRequired(failure)
    } else {
        RecoveryError::RecoveryRequired(failure)
    }
}
fn inspect_protocol(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
) -> Result<super::protocol::Journal, RecoveryError> {
    let journal = super::protocol::inspect(directory)
        .map_err(|source| protocol_error(project, id, source))?;
    journal
        .validate_project(project)
        .map_err(|source| protocol_error(project, id, source))?;
    Ok(journal)
}

fn preflight_and_reject_overlapping_targets<S: RecoveryScope>(
    project: &LockedProject<'_>,
    transactions: &BTreeMap<TransactionId, PathBuf>,
    scope: &S,
) -> Result<(), RecoveryError> {
    let mut owners = BTreeMap::<ProjectRelativePath, TransactionId>::new();
    for (id, directory) in transactions {
        validate_journal_tree(project, id, directory)?;
        let journal = inspect_protocol(project, id, directory)?;
        if journal.cleaning() {
            super::protocol::verify_cleanup(project, directory)
                .map_err(|source| protocol_error(project, id, source))?;
            if let Some(manifest) = &journal.manifest {
                scope
                    .validate_manifest(id, directory, manifest)
                    .map_err(|reason| scoped_manual(project, id, reason))?;
            } else {
                scope
                    .validate_missing_manifest(id, directory)
                    .map_err(|reason| scoped_manual(project, id, reason))?;
            }
            continue;
        }
        if matches!(journal.mode, super::protocol::Mode::OwnedPreparing) {
            super::protocol::verify_preparing(project, directory)
                .map_err(|source| protocol_error(project, id, source))?;
            if let Some(targets) = journal
                .state
                .as_ref()
                .and_then(|s| s.original_targets.as_ref())
            {
                for target in targets {
                    if owners
                        .insert(target.target_path.clone(), id.clone())
                        .is_some()
                    {
                        return Err(manual(
                            project,
                            Some(id),
                            Some(&target.target_path),
                            RecoveryStage::InspectJournal,
                            "pending transactions have overlapping targets",
                        ));
                    }
                }
            }
            if let Some(manifest) = &journal.manifest {
                scope
                    .validate_manifest(id, directory, manifest)
                    .map_err(|reason| scoped_manual(project, id, reason))?;
            } else {
                scope
                    .validate_missing_manifest(id, directory)
                    .map_err(|reason| scoped_manual(project, id, reason))?;
            }
            continue;
        }
        let committed =
            private_artifact_exists(project, id, directory, &directory.join("committed.json"))?;
        let rolled_back =
            private_artifact_exists(project, id, directory, &directory.join("rolled-back.json"))?;
        if committed && rolled_back {
            return Err(manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "both completion markers exist",
            ));
        }
        if committed {
            let marker: CommittedMarker =
                read_valid_json(project, id, directory, &directory.join("committed.json"))?;
            marker.validate().map_err(|_| {
                manual(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    "invalid committed marker",
                )
            })?;
            verify_identity(
                project,
                id,
                &marker.transaction_id,
                &marker.project_fingerprint,
            )?;
        }
        if rolled_back {
            let marker: RolledBackMarker =
                read_valid_json(project, id, directory, &directory.join("rolled-back.json"))?;
            marker.validate().map_err(|_| {
                manual(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    "invalid rolled-back marker",
                )
            })?;
            verify_identity(
                project,
                id,
                &marker.transaction_id,
                &marker.project_fingerprint,
            )?;
        }
        let Some(manifest) = journal.manifest else {
            scope
                .validate_missing_manifest(id, directory)
                .map_err(|reason| scoped_manual(project, id, reason))?;
            if !is_unambiguous_preparing_orphan(project, id, directory)? {
                return Err(manual(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    "ambiguous missing manifest",
                ));
            }
            continue;
        };
        preflight_manifest_artifacts(project, id, directory, &manifest)?;
        scope
            .validate_manifest(id, directory, &manifest)
            .map_err(|reason| scoped_manual(project, id, reason))?;
        for operation in &manifest.operations {
            if owners
                .insert(operation.target_path.clone(), id.clone())
                .is_some()
            {
                return Err(manual(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::InspectJournal,
                    "pending transactions have overlapping targets",
                ));
            }
        }
    }
    Ok(())
}

/// mutation 전에 journal 전체의 허용된 topology와 파일 handle 경계를 확인한다.
/// 내용이 필요 없는 state도 실제 regular file handle로 열어 link 우회를 차단한다.
pub(super) fn validate_journal_tree(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
) -> Result<(), RecoveryError> {
    let parent = directory.parent().ok_or_else(|| {
        manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "transaction directory parent is missing",
        )
    })?;
    verify_transaction_directory(project, id, parent, directory)?;
    let journal = inspect_protocol(project, id, directory)?;

    let entries = fs::read_dir(directory).map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            source,
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                source,
            )
        })?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "non-UTF-8 journal artifact name",
            ));
        };
        match name {
            "state.json" | "manifest.json" | "committed.json" | "rolled-back.json" => {
                open_private_artifact_required(project, id, directory, &entry.path())?;
            }
            "staged" | "backups" => {
                validate_artifact_directory(project, id, directory, &entry.path())?;
            }
            "owned-temp-v1" => {
                let result = if journal.cleaning() {
                    super::protocol::validate_cleanup_records(directory)
                } else if journal.mode == super::protocol::Mode::OwnedPreparing {
                    Ok(())
                } else {
                    let manifest = journal.manifest.as_ref().ok_or_else(|| {
                        manual(
                            project,
                            Some(id),
                            None,
                            RecoveryStage::InspectJournal,
                            "manifest is required",
                        )
                    })?;
                    super::owned_temp::validate_tree(project, id, directory, manifest)
                };
                result.map_err(|source| {
                    required(
                        project,
                        Some(id),
                        None,
                        RecoveryStage::InspectJournal,
                        source,
                    )
                })?;
            }
            _ => {
                return Err(manual(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    "unknown transaction journal artifact",
                ));
            }
        }
    }
    Ok(())
}

fn validate_artifact_directory(
    project: &LockedProject<'_>,
    id: &TransactionId,
    transaction_directory: &Path,
    artifact_directory: &Path,
) -> Result<(), RecoveryError> {
    let directory = verify_real_directory(artifact_directory, Some(transaction_directory))
        .map_err(|source| {
            namespace_error(project, Some(id), RecoveryStage::InspectJournal, source)
        })?
        .ok_or_else(|| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                io::Error::new(io::ErrorKind::NotFound, "artifact directory disappeared"),
            )
        })?;
    let entries = fs::read_dir(&directory).map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            source,
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                source,
            )
        })?;
        let name = entry.file_name();
        let valid_name = name.to_str().is_some_and(is_indexed_artifact_name);
        if !valid_name {
            return Err(manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "invalid transaction artifact name",
            ));
        }
        read_private_artifact_required(project, id, transaction_directory, &entry.path())?;
    }
    Ok(())
}

fn is_indexed_artifact_name(name: &str) -> bool {
    name.len() == 11
        && name.ends_with(".json")
        && name[..6].bytes().all(|byte| byte.is_ascii_digit())
}

fn preflight_manifest_artifacts(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    manifest: &TransactionManifest,
) -> Result<(), RecoveryError> {
    for operation in &manifest.operations {
        if let Some(staged) = read_private_artifact_optional(
            project,
            id,
            directory,
            &directory.join(&operation.staged_path),
        )? {
            verify_bytes(
                Some(operation.staged_size),
                Some(&operation.staged_sha256),
                operation.staged_schema_version,
                &staged,
                false,
            )
            .map_err(|reason| {
                manual(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::InspectJournal,
                    reason,
                )
            })?;
        }
        if operation.original_existed {
            let backup_path = operation.backup_path.as_deref().ok_or_else(|| {
                manual(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::InspectJournal,
                    "backup path missing",
                )
            })?;
            let backup = read_private_artifact_required(
                project,
                id,
                directory,
                &directory.join(backup_path),
            )?;
            verify_bytes(
                operation.original_size,
                operation.original_sha256.as_deref(),
                operation.original_schema_version,
                &backup,
                operation.original_raw,
            )
            .map_err(|reason| {
                manual(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::InspectJournal,
                    reason,
                )
            })?;
        }
    }
    Ok(())
}

fn recover_one<H: RecoveryHooks, S: RecoveryScope>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    hooks: &H,
    scope: &S,
) -> Result<RecoveredKind, RecoveryError> {
    let journal = inspect_protocol(project, id, directory)?;
    if journal.cleaning() {
        validate_journal_tree(project, id, directory)?;
        super::protocol::finish_cleanup(project, directory).map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::CleanupTransaction,
                source,
            )
        })?;
        return Ok(
            if journal.mode == super::protocol::Mode::CleaningCommitted {
                RecoveredKind::CommittedCleanup
            } else {
                RecoveredKind::RolledBackCleanup
            },
        );
    }
    if journal.mode == super::protocol::Mode::OwnedPreparing {
        if let Some(manifest) = &journal.manifest {
            scope
                .validate_manifest(id, directory, manifest)
                .map_err(|reason| scoped_manual(project, id, reason))?;
        } else {
            scope
                .validate_missing_manifest(id, directory)
                .map_err(|reason| scoped_manual(project, id, reason))?;
        }
        super::protocol::cleanup_preparing(project, directory).map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::CleanupTransaction,
                source,
            )
        })?;
        return Ok(RecoveredKind::PreparingCleanup);
    }
    let guard_manifest = if journal.mode == super::protocol::Mode::OwnedActive {
        journal.manifest.as_ref()
    } else {
        None
    };
    let _owned_guards = if let Some(manifest) = &guard_manifest {
        super::owned_temp::guards(project, directory, manifest).map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                source,
            )
        })?
    } else {
        Vec::new()
    };
    let committed_exists =
        private_artifact_exists(project, id, directory, &directory.join("committed.json"))?;
    let rolled_back_exists =
        private_artifact_exists(project, id, directory, &directory.join("rolled-back.json"))?;
    if committed_exists && rolled_back_exists {
        return Err(manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "both completion markers exist",
        ));
    }

    if committed_exists {
        let marker: CommittedMarker =
            read_valid_json(project, id, directory, &directory.join("committed.json"))?;
        marker.validate().map_err(|_| {
            manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "invalid committed marker",
            )
        })?;
        verify_identity(
            project,
            id,
            &marker.transaction_id,
            &marker.project_fingerprint,
        )?;
        let manifest = journal.manifest.as_ref().ok_or_else(|| {
            manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "manifest is required",
            )
        })?;
        scope
            .validate_manifest(id, directory, manifest)
            .map_err(|reason| scoped_manual(project, id, reason))?;
        for operation in &manifest.operations {
            hooks
                .check(
                    RecoveryFailPoint::CommittedTargetVerify,
                    Some(operation.index),
                )
                .map_err(|source| {
                    required(
                        project,
                        Some(id),
                        Some(&operation.target_path),
                        RecoveryStage::VerifyCommittedTarget,
                        source,
                    )
                })?;
            verify_target_matches(
                project,
                id,
                operation,
                true,
                RecoveryStage::VerifyCommittedTarget,
            )?;
        }
        cleanup(project, id, directory, hooks, true)?;
        return Ok(RecoveredKind::CommittedCleanup);
    }

    if rolled_back_exists {
        let marker: RolledBackMarker =
            read_valid_json(project, id, directory, &directory.join("rolled-back.json"))?;
        marker.validate().map_err(|_| {
            manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "invalid rolled-back marker",
            )
        })?;
        verify_identity(
            project,
            id,
            &marker.transaction_id,
            &marker.project_fingerprint,
        )?;
        let manifest = journal.manifest.as_ref().ok_or_else(|| {
            manual(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                "manifest is required",
            )
        })?;
        scope
            .validate_manifest(id, directory, manifest)
            .map_err(|reason| scoped_manual(project, id, reason))?;
        verify_all_original(project, id, manifest)?;
        cleanup(project, id, directory, hooks, true)?;
        return Ok(RecoveredKind::RolledBackCleanup);
    }

    match journal.manifest {
        Some(manifest) => {
            scope
                .validate_manifest(id, directory, &manifest)
                .map_err(|reason| scoped_manual(project, id, reason))?;
            rollback_manifest(project, id, directory, &manifest, hooks)?;
            Ok(RecoveredKind::RolledBack)
        }
        None => {
            scope
                .validate_missing_manifest(id, directory)
                .map_err(|reason| scoped_manual(project, id, reason))?;
            if is_unambiguous_preparing_orphan(project, id, directory)? {
                cleanup(project, id, directory, hooks, false)?;
                Ok(RecoveredKind::PreparingCleanup)
            } else {
                Err(manual(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    "manifest is missing and apply cannot be excluded",
                ))
            }
        }
    }
}

pub(super) fn rollback_manifest<H: RecoveryHooks>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    manifest: &TransactionManifest,
    hooks: &H,
) -> Result<(), RecoveryError> {
    verify_manifest_identity(project, id, manifest)?;
    validate_journal_tree(project, id, directory)?;
    let journal = inspect_protocol(project, id, directory)?;
    if journal.manifest.as_ref() != Some(manifest)
        || journal.cleaning()
        || journal.committed
        || journal.rolled_back
    {
        return Err(manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "rollback protocol or completion changed",
        ));
    }
    let _owned_guards =
        super::owned_temp::guards(project, directory, manifest).map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                source,
            )
        })?;
    super::owned_temp::cleanup(project, id, directory, manifest).map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::CleanupTransaction,
            source,
        )
    })?;
    write_state(
        project,
        id,
        directory,
        TransactionState::RollingBack,
        &full_prefix(manifest.operations.len()),
        hooks,
        (
            RecoveryFailPoint::RollingBackState,
            RecoveryStage::WriteRollingBackState,
        ),
    )?;

    for operation in manifest.operations.iter().rev() {
        rollback_operation(project, id, directory, operation, hooks)?;
        let remaining = usize::try_from(operation.index).unwrap_or(0);
        write_state(
            project,
            id,
            directory,
            TransactionState::RollingBack,
            &full_prefix(remaining),
            hooks,
            (
                RecoveryFailPoint::ProgressState,
                RecoveryStage::WriteRollbackProgress,
            ),
        )?;
    }
    verify_all_original(project, id, manifest)?;
    write_rolled_back_marker(project, id, directory, hooks)?;
    if let Err(error) = write_state(
        project,
        id,
        directory,
        TransactionState::RolledBack,
        &[],
        hooks,
        (
            RecoveryFailPoint::RolledBackState,
            RecoveryStage::WriteRolledBackState,
        ),
    ) {
        return Err(cleanup_completed(error));
    }
    cleanup(project, id, directory, hooks, true)
}

fn rollback_operation<H: RecoveryHooks>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    operation: &TransactionOperation,
    hooks: &H,
) -> Result<(), RecoveryError> {
    let target = resolve_target(project, id, &operation.target_path)?;
    if operation.original_existed {
        hooks
            .check(RecoveryFailPoint::BackupVerify, Some(operation.index))
            .map_err(|source| {
                required(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::VerifyBackup,
                    source,
                )
            })?;
        let backup_path = operation.backup_path.as_deref().ok_or_else(|| {
            manual(
                project,
                Some(id),
                Some(&operation.target_path),
                RecoveryStage::VerifyBackup,
                "backup path missing",
            )
        })?;
        let backup =
            read_private_artifact_required(project, id, directory, &directory.join(backup_path))?;
        verify_bytes(
            operation.original_size,
            operation.original_sha256.as_deref(),
            operation.original_schema_version,
            &backup,
            operation.original_raw,
        )
        .map_err(|reason| {
            manual(
                project,
                Some(id),
                Some(&operation.target_path),
                RecoveryStage::VerifyBackup,
                reason,
            )
        })?;
        match classify_target(&target, operation)? {
            TargetState::Original => {}
            TargetState::Staged | TargetState::Missing => {
                hooks
                    .check(RecoveryFailPoint::RestoreExisting, Some(operation.index))
                    .map_err(|source| {
                        required(
                            project,
                            Some(id),
                            Some(&operation.target_path),
                            RecoveryStage::RestoreExistingTarget,
                            source,
                        )
                    })?;
                if super::owned_temp::enabled(directory).map_err(|source| {
                    required(
                        project,
                        Some(id),
                        Some(&operation.target_path),
                        RecoveryStage::RestoreExistingTarget,
                        source,
                    )
                })? {
                    super::owned_temp::restore(project, id, directory, operation).map_err(
                        |source| {
                            required(
                                project,
                                Some(id),
                                Some(&operation.target_path),
                                RecoveryStage::RestoreExistingTarget,
                                source,
                            )
                        },
                    )?;
                } else {
                    save_bytes_atomic(&target, &backup).map_err(|source| {
                        required(
                            project,
                            Some(id),
                            Some(&operation.target_path),
                            RecoveryStage::RestoreExistingTarget,
                            io::Error::other(source),
                        )
                    })?;
                }
            }
            TargetState::Other => {
                return Err(manual(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::RestoreExistingTarget,
                    "target differs from original and staged content",
                ))
            }
        }
    } else {
        match classify_target(&target, operation)? {
            TargetState::Missing => {}
            TargetState::Staged => {
                hooks
                    .check(RecoveryFailPoint::RemoveNew, Some(operation.index))
                    .map_err(|source| {
                        required(
                            project,
                            Some(id),
                            Some(&operation.target_path),
                            RecoveryStage::RemoveNewTarget,
                            source,
                        )
                    })?;
                if super::owned_temp::enabled(directory).map_err(|source| {
                    required(
                        project,
                        Some(id),
                        Some(&operation.target_path),
                        RecoveryStage::RemoveNewTarget,
                        source,
                    )
                })? {
                    super::owned_temp::remove_new(project, id, directory, operation)
                } else {
                    fs::remove_file(&target)
                }
                .map_err(|source| {
                    required(
                        project,
                        Some(id),
                        Some(&operation.target_path),
                        RecoveryStage::RemoveNewTarget,
                        source,
                    )
                })?;
                sync_directory(target.parent().ok_or_else(|| {
                    manual(
                        project,
                        Some(id),
                        Some(&operation.target_path),
                        RecoveryStage::RemoveNewTarget,
                        "target parent missing",
                    )
                })?)
                .map_err(|source| {
                    required(
                        project,
                        Some(id),
                        Some(&operation.target_path),
                        RecoveryStage::RemoveNewTarget,
                        source,
                    )
                })?;
            }
            TargetState::Original | TargetState::Other => {
                return Err(manual(
                    project,
                    Some(id),
                    Some(&operation.target_path),
                    RecoveryStage::RemoveNewTarget,
                    "new target has unrelated content",
                ))
            }
        }
    }
    hooks
        .check(RecoveryFailPoint::RestoredVerify, Some(operation.index))
        .map_err(|source| {
            required(
                project,
                Some(id),
                Some(&operation.target_path),
                RecoveryStage::VerifyRestoredTarget,
                source,
            )
        })?;
    verify_target_matches(
        project,
        id,
        operation,
        false,
        RecoveryStage::VerifyRestoredTarget,
    )
}

#[derive(Clone, Copy)]
pub(super) enum TargetState {
    Original,
    Staged,
    Missing,
    Other,
}

pub(super) fn classify_target(
    path: &Path,
    operation: &TransactionOperation,
) -> Result<TargetState, RecoveryError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(TargetState::Missing),
        Err(source) => {
            return Err(RecoveryError::RecoveryRequired(Box::new(RecoveryFailure {
                stage: RecoveryStage::InspectJournal,
                transaction_id: None,
                project_fingerprint: "unavailable".to_owned(),
                target: Some(Box::new(operation.target_path.clone())),
                source,
            })))
        }
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(TargetState::Other);
    }
    let bytes = fs::read(path).map_err(|source| {
        RecoveryError::RecoveryRequired(Box::new(RecoveryFailure {
            stage: RecoveryStage::InspectJournal,
            transaction_id: None,
            project_fingerprint: "unavailable".to_owned(),
            target: Some(Box::new(operation.target_path.clone())),
            source,
        }))
    })?;
    let hash = sha256(&bytes);
    if operation.original_existed
        && operation.original_size == Some(bytes.len() as u64)
        && operation.original_sha256.as_deref() == Some(&hash)
    {
        return Ok(TargetState::Original);
    }
    if operation.staged_size == bytes.len() as u64 && operation.staged_sha256 == hash {
        return Ok(TargetState::Staged);
    }
    Ok(TargetState::Other)
}

fn verify_target_matches(
    project: &LockedProject<'_>,
    id: &TransactionId,
    operation: &TransactionOperation,
    staged: bool,
    stage: RecoveryStage,
) -> Result<(), RecoveryError> {
    let target = resolve_target(project, id, &operation.target_path)?;
    let state = classify_target(&target, operation)?;
    let matches = if staged {
        matches!(state, TargetState::Staged)
    } else if operation.original_existed {
        matches!(state, TargetState::Original)
    } else {
        matches!(state, TargetState::Missing)
    };
    if matches {
        Ok(())
    } else {
        Err(manual(
            project,
            Some(id),
            Some(&operation.target_path),
            stage,
            "target does not match required transaction state",
        ))
    }
}

fn verify_all_original(
    project: &LockedProject<'_>,
    id: &TransactionId,
    manifest: &TransactionManifest,
) -> Result<(), RecoveryError> {
    for operation in &manifest.operations {
        verify_target_matches(
            project,
            id,
            operation,
            false,
            RecoveryStage::VerifyRolledBackTarget,
        )?;
    }
    Ok(())
}

fn write_rolled_back_marker<H: RecoveryHooks>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    hooks: &H,
) -> Result<(), RecoveryError> {
    let completed_at_utc = now_utc_milliseconds().map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::WriteRolledBackMarker,
            io::Error::other(source),
        )
    })?;
    let marker = RolledBackMarker {
        schema_version: inspect_protocol(project, id, directory)?
            .version()
            .map_err(|source| {
                required(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    source,
                )
            })?,
        transaction_id: id.clone(),
        completed_at_utc,
        project_fingerprint: project.fingerprint().to_owned(),
    };
    hooks
        .check(RecoveryFailPoint::RolledBackMarkerWrite, None)
        .map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::WriteRolledBackMarker,
                source,
            )
        })?;
    save_deterministic_json(&directory.join("rolled-back.json"), &marker).map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::WriteRolledBackMarker,
            io::Error::other(source),
        )
    })?;
    hooks
        .check(RecoveryFailPoint::RolledBackMarkerVerify, None)
        .map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::VerifyRolledBackMarker,
                source,
            )
        })?;
    let decoded: RolledBackMarker =
        read_valid_json(project, id, directory, &directory.join("rolled-back.json"))?;
    decoded.validate().map_err(|_| {
        manual(
            project,
            Some(id),
            None,
            RecoveryStage::VerifyRolledBackMarker,
            "invalid rolled-back marker",
        )
    })?;
    if decoded != marker {
        return Err(manual(
            project,
            Some(id),
            None,
            RecoveryStage::VerifyRolledBackMarker,
            "rolled-back marker identity changed",
        ));
    }
    hooks
        .check(RecoveryFailPoint::RolledBackMarkerSync, None)
        .and_then(|()| {
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(directory.join("rolled-back.json"))?
                .sync_all()
        })
        .and_then(|()| sync_directory(directory))
        .map_err(|source| {
            required(
                project,
                Some(id),
                None,
                RecoveryStage::SyncRolledBackMarker,
                source,
            )
        })
}

fn write_state<H: RecoveryHooks>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    state: TransactionState,
    progress: &[u32],
    hooks: &H,
    boundary: (RecoveryFailPoint, RecoveryStage),
) -> Result<(), RecoveryError> {
    let (point, stage) = boundary;
    let updated_at_utc = now_utc_milliseconds()
        .map_err(|source| required(project, Some(id), None, stage, io::Error::other(source)))?;
    let record = TransactionStateRecord {
        schema_version: inspect_protocol(project, id, directory)?
            .version()
            .map_err(|source| {
                required(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    source,
                )
            })?,
        transaction_id: id.clone(),
        updated_at_utc,
        project_fingerprint: project.fingerprint().to_owned(),
        state,
        applied_operations: progress.to_vec(),
        original_targets: inspect_protocol(project, id, directory)?
            .state
            .and_then(|state| state.original_targets),
    };
    hooks
        .check(point, progress.last().copied())
        .map_err(|source| required(project, Some(id), None, stage, source))?;
    save_deterministic_json(&directory.join("state.json"), &record)
        .map_err(|source| required(project, Some(id), None, stage, io::Error::other(source)))
}

fn cleanup<H: RecoveryHooks>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    hooks: &H,
    completed: bool,
) -> Result<(), RecoveryError> {
    // `remove_dir_all` 직전에 namespace와 entry identity를 다시 확인한다. 입증할 수
    // 없으면 journal을 그대로 보존하고 fail closed한다.
    let namespace = verify_transaction_namespace(project)?.ok_or_else(|| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::CleanupTransaction,
            io::Error::new(io::ErrorKind::NotFound, "transaction namespace disappeared"),
        )
    })?;
    let verified_directory = verify_transaction_directory(project, id, &namespace.root, directory)?;
    validate_journal_tree(project, id, &verified_directory)?;
    let result = hooks
        .check(RecoveryFailPoint::Cleanup, None)
        .and_then(|()| {
            let journal = super::protocol::inspect(directory)?;
            journal.validate_project(project)?;
            if journal.mode == super::protocol::Mode::OwnedActive || journal.cleaning() {
                super::protocol::certify_cleanup(project, directory)
            } else if journal.mode == super::protocol::Mode::OwnedPreparing {
                super::protocol::cleanup_preparing(project, directory)
            } else {
                fs::remove_dir_all(&verified_directory)?;
                sync_directory(&namespace.root)
            }
        });
    result.map_err(|source| {
        let error = required(
            project,
            Some(id),
            None,
            RecoveryStage::CleanupTransaction,
            source,
        );
        if completed {
            cleanup_completed(error)
        } else {
            error
        }
    })
}

pub(super) fn cleanup_committed(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
) -> io::Result<()> {
    let journal = inspect_protocol(project, id, directory).map_err(io::Error::other)?;
    if !journal.committed || journal.rolled_back {
        return Err(super::protocol::invalid("committed cleanup marker missing"));
    }
    validate_journal_tree(project, id, directory).map_err(io::Error::other)?;
    if journal.cleaning() {
        return super::protocol::finish_cleanup(project, directory);
    }
    let manifest = read_manifest(project, id, directory).map_err(io::Error::other)?;
    let _guards = super::owned_temp::guards(project, directory, &manifest)?;
    for op in &manifest.operations {
        verify_target_matches(project, id, op, true, RecoveryStage::VerifyCommittedTarget)
            .map_err(io::Error::other)?;
    }
    cleanup(project, id, directory, &NoRecoveryHooks, true).map_err(io::Error::other)
}

fn cleanup_completed(error: RecoveryError) -> RecoveryError {
    // 상태만 바꾼다. 문자열로 재생성하면 원 OS code와 custom cause 소유권이 사라진다.
    let failure = match error {
        RecoveryError::RolledBackCleanupFailed(failure)
        | RecoveryError::ManualRecoveryRequired(failure)
        | RecoveryError::RecoveryRequired(failure) => failure,
    };
    RecoveryError::RolledBackCleanupFailed(failure)
}

fn read_manifest(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
) -> Result<TransactionManifest, RecoveryError> {
    read_manifest_optional(project, id, directory)?.ok_or_else(|| {
        manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "manifest is required",
        )
    })
}

fn read_manifest_optional(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
) -> Result<Option<TransactionManifest>, RecoveryError> {
    let path = directory.join("manifest.json");
    let Some(mut file) = open_private_artifact_optional(project, id, directory, &path)? else {
        return Ok(None);
    };
    let manifest: TransactionManifest = super::protocol::read_main(&mut file)
        .map_err(|source| protocol_error(project, id, source))?;
    manifest.validate().map_err(|_| {
        manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "manifest validation failed or schema is unsupported",
        )
    })?;
    verify_manifest_identity(project, id, &manifest)?;
    Ok(Some(manifest))
}

fn read_valid_json<T: MainRecord>(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    path: &Path,
) -> Result<T, RecoveryError> {
    let mut file = open_private_artifact_required(project, id, directory, path)?;
    super::protocol::read_main(&mut file).map_err(|source| protocol_error(project, id, source))
}

fn private_artifact_exists(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    path: &Path,
) -> Result<bool, RecoveryError> {
    open_private_artifact_optional(project, id, directory, path).map(|value| value.is_some())
}

fn read_private_artifact_required(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    path: &Path,
) -> Result<Vec<u8>, RecoveryError> {
    read_private_artifact_optional(project, id, directory, path)?.ok_or_else(|| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            io::Error::new(
                io::ErrorKind::NotFound,
                "required journal artifact is missing",
            ),
        )
    })
}

// staged/backup은 기존 hash/schema 검증이 raw bytes를 요구한다. 주 기록은 이 helper를 사용하지 않는다.
fn read_private_artifact_optional(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    path: &Path,
) -> Result<Option<Vec<u8>>, RecoveryError> {
    let Some(mut file) = open_private_artifact_optional(project, id, directory, path)? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            source,
        )
    })?;
    Ok(Some(bytes))
}
fn open_private_artifact_required(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    path: &Path,
) -> Result<fs::File, RecoveryError> {
    open_private_artifact_optional(project, id, directory, path)?.ok_or_else(|| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            io::Error::new(
                io::ErrorKind::NotFound,
                "required journal artifact is missing",
            ),
        )
    })
}

fn open_private_artifact_optional(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
    path: &Path,
) -> Result<Option<fs::File>, RecoveryError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(required(
                project,
                Some(id),
                None,
                RecoveryStage::InspectJournal,
                source,
            ));
        }
    };
    if !is_real_file(&metadata) {
        return Err(manual(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            "journal artifact is not a regular non-reparse file",
        ));
    }

    open_existing_private_file(directory, path)
        .map(Some)
        .map_err(|source| namespace_error(project, Some(id), RecoveryStage::InspectJournal, source))
}

fn verify_manifest_identity(
    project: &LockedProject<'_>,
    id: &TransactionId,
    manifest: &TransactionManifest,
) -> Result<(), RecoveryError> {
    verify_identity(
        project,
        id,
        &manifest.transaction_id,
        &manifest.project_fingerprint,
    )
}

fn verify_identity(
    project: &LockedProject<'_>,
    directory_id: &TransactionId,
    recorded_id: &TransactionId,
    fingerprint: &str,
) -> Result<(), RecoveryError> {
    if directory_id != recorded_id || fingerprint != project.fingerprint() {
        return Err(manual(
            project,
            Some(directory_id),
            None,
            RecoveryStage::InspectJournal,
            "journal identity does not match transaction directory or project",
        ));
    }
    Ok(())
}

fn is_unambiguous_preparing_orphan(
    project: &LockedProject<'_>,
    id: &TransactionId,
    directory: &Path,
) -> Result<bool, RecoveryError> {
    let state_path = directory.join("state.json");
    if private_artifact_exists(project, id, directory, &state_path)? {
        let state: TransactionStateRecord = read_valid_json(project, id, directory, &state_path)?;
        if state.validate().is_err()
            || state.transaction_id != *id
            || state.project_fingerprint != project.fingerprint()
            || state.state != TransactionState::Preparing
            || !state.applied_operations.is_empty()
        {
            return Ok(false);
        }
    }
    for entry in fs::read_dir(directory).map_err(|source| {
        required(
            project,
            Some(id),
            None,
            RecoveryStage::InspectJournal,
            source,
        )
    })? {
        let name = entry
            .map_err(|source| {
                required(
                    project,
                    Some(id),
                    None,
                    RecoveryStage::InspectJournal,
                    source,
                )
            })?
            .file_name();
        let Some(name) = name.to_str() else {
            return Ok(false);
        };
        if !matches!(name, "state.json" | "staged" | "backups") {
            return Ok(false);
        }
    }
    Ok(true)
}

fn resolve_target(
    project: &LockedProject<'_>,
    id: &TransactionId,
    target: &ProjectRelativePath,
) -> Result<PathBuf, RecoveryError> {
    let absolute = project.canonical_root().join(target.as_str());
    let parent = absolute.parent().ok_or_else(|| {
        manual(
            project,
            Some(id),
            Some(target),
            RecoveryStage::InspectJournal,
            "target parent missing",
        )
    })?;
    let metadata = fs::symlink_metadata(parent).map_err(|source| {
        required(
            project,
            Some(id),
            Some(target),
            RecoveryStage::InspectJournal,
            source,
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(manual(
            project,
            Some(id),
            Some(target),
            RecoveryStage::InspectJournal,
            "target parent is not a real directory",
        ));
    }
    let canonical = fs::canonicalize(parent).map_err(|source| {
        required(
            project,
            Some(id),
            Some(target),
            RecoveryStage::InspectJournal,
            source,
        )
    })?;
    if !canonical.starts_with(project.canonical_root()) {
        return Err(manual(
            project,
            Some(id),
            Some(target),
            RecoveryStage::InspectJournal,
            "target parent resolves outside project",
        ));
    }
    Ok(absolute)
}

fn verify_bytes(
    expected_size: Option<u64>,
    expected_hash: Option<&str>,
    expected_schema: Option<super::super::schema::SchemaVersion>,
    bytes: &[u8],
    raw: bool,
) -> Result<(), &'static str> {
    if expected_size != Some(bytes.len() as u64) || expected_hash != Some(sha256(bytes).as_str()) {
        return Err("artifact size or SHA-256 differs from manifest");
    }
    if raw {
        return Ok(());
    }
    let schema =
        managed_schema_version(bytes).map_err(|_| "artifact JSON or schemaVersion is invalid")?;
    if expected_schema.is_some_and(|value| value != schema) {
        return Err("artifact schemaVersion differs from manifest");
    }
    Ok(())
}

fn full_prefix(length: usize) -> Vec<u32> {
    (0..length)
        .filter_map(|value| u32::try_from(value).ok())
        .collect()
}

fn required(
    project: &LockedProject<'_>,
    id: Option<&TransactionId>,
    target: Option<&ProjectRelativePath>,
    stage: RecoveryStage,
    source: io::Error,
) -> RecoveryError {
    RecoveryError::RecoveryRequired(Box::new(RecoveryFailure {
        stage,
        transaction_id: id.cloned(),
        project_fingerprint: project.fingerprint().to_owned(),
        target: target.cloned().map(Box::new),
        source,
    }))
}

fn manual(
    project: &LockedProject<'_>,
    id: Option<&TransactionId>,
    target: Option<&ProjectRelativePath>,
    stage: RecoveryStage,
    reason: &'static str,
) -> RecoveryError {
    RecoveryError::ManualRecoveryRequired(Box::new(RecoveryFailure {
        stage,
        transaction_id: id.cloned(),
        project_fingerprint: project.fingerprint().to_owned(),
        target: target.cloned().map(Box::new),
        source: io::Error::new(io::ErrorKind::InvalidData, reason),
    }))
}

fn scoped_manual(
    project: &LockedProject<'_>,
    id: &TransactionId,
    reason: &'static str,
) -> RecoveryError {
    manual(
        project,
        Some(id),
        None,
        RecoveryStage::InspectJournal,
        reason,
    )
}

#[cfg(test)]
pub(super) fn fix005_wrap(
    project: &LockedProject<'_>,
    id: &TransactionId,
    error: io::Error,
) -> RecoveryError {
    protocol_error(project, id, error)
}
