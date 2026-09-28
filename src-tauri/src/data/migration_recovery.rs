use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use super::collaboration_lock::{WritePermit, WritePermitError};
use super::migration::strict_schema_version;
use super::migration_prepare::MigrationVersionTransition;
use super::project_file::{open_existing_private_file, open_existing_project_file};
use super::project_relative_path::ProjectRelativePath;
#[cfg(not(test))]
use super::transaction::recover_pending_transactions_with_scope;
use super::transaction::{
    LockedProject, RecoveryError, RecoveryReport, RecoveryScope, TransactionId,
    TransactionManifest, TransactionOperation, TransactionState, TransactionStateRecord,
};

pub(crate) struct MigrationRecoveryRequest {
    pub(crate) targets: Vec<ProjectRelativePath>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecoveredMigrationTransaction {
    pub(crate) transaction_id: TransactionId,
    pub(crate) transitions: Vec<MigrationVersionTransition>,
}

#[derive(Debug)]
pub(crate) struct MigrationRecoveryReport {
    pub(crate) project_fingerprint: String,
    pub(crate) targets: Vec<ProjectRelativePath>,
    pub(crate) migration: Option<RecoveredMigrationTransaction>,
    /// 기존 recovery의 집계와 완료 종류를 손실 없이 보존한다.
    pub(crate) transaction: RecoveryReport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationRecoveryStage {
    InvalidTargetSet,
    PermitValidation,
    InspectMigrationJournal,
    TransactionRecovery,
}

pub(crate) enum MigrationRecoveryErrorSource {
    Permit(WritePermitError),
    Transaction(RecoveryError),
}

pub(crate) struct MigrationRecoveryError {
    pub(crate) stage: MigrationRecoveryStage,
    pub(crate) project_fingerprint: String,
    pub(crate) target: Option<Box<ProjectRelativePath>>,
    pub(crate) source: Option<Box<MigrationRecoveryErrorSource>>,
}

impl fmt::Display for MigrationRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "migration recovery failed at {:?} for project [redacted]",
            self.stage
        )?;
        if let Some(target) = &self.target {
            write!(formatter, ", target '{target}'")?;
        }
        formatter.write_str("; preserve transaction artifacts and retry after resolving the cause")
    }
}

impl fmt::Debug for MigrationRecoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MigrationRecoveryError")
            .field("stage", &self.stage)
            .field("project", &"[redacted]")
            .field("target", &self.target)
            .field("has_source", &self.source.is_some())
            .finish()
    }
}

impl Error for MigrationRecoveryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn Error + 'static))
    }
}

impl fmt::Display for MigrationRecoveryErrorSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Permit(_) => formatter.write_str("migration recovery permit validation failed"),
            Self::Transaction(_) => formatter.write_str("underlying transaction recovery failed"),
        }
    }
}

impl fmt::Debug for MigrationRecoveryErrorSource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Permit(_) => "MigrationRecoveryErrorSource::Permit([redacted])",
            Self::Transaction(_) => "MigrationRecoveryErrorSource::Transaction([redacted])",
        })
    }
}

impl Error for MigrationRecoveryErrorSource {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        // 원본 typed source는 enum 필드로 보존하지만 자동 formatting chain에는
        // fingerprint를 포함할 수 있는 하위 오류를 연결하지 않는다.
        None
    }
}

pub(crate) fn recover_migration<'guard>(
    mut request: MigrationRecoveryRequest,
    project: &LockedProject<'_>,
    mut permit: WritePermit<'guard>,
) -> Result<MigrationRecoveryReport, MigrationRecoveryError> {
    validate_recovery_request(&mut request, project, &mut permit)?;
    let scope = MigrationRecoveryScope::new(project, &request.targets);
    #[cfg(test)]
    let transaction =
        super::transaction::test_support::recover_with_configured_test_hooks_and_scope(
            project, &scope,
        )
        .map_err(|source| map_recovery_error(project, source))?;
    #[cfg(not(test))]
    let transaction = recover_pending_transactions_with_scope(project, &scope)
        .map_err(|source| map_recovery_error(project, source))?;
    Ok(finish_recovery_report(project, request, scope, transaction))
}

fn validate_recovery_request(
    request: &mut MigrationRecoveryRequest,
    project: &LockedProject<'_>,
    permit: &mut WritePermit<'_>,
) -> Result<(), MigrationRecoveryError> {
    request.targets.sort();
    if request.targets.is_empty() || request.targets.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(MigrationRecoveryError {
            stage: MigrationRecoveryStage::InvalidTargetSet,
            project_fingerprint: project.fingerprint().to_owned(),
            target: None,
            source: None,
        });
    }

    // journal에 접근하기 전에 exact permit을 먼저 검사해 권한 실패가 mutation은 물론
    // transaction filesystem 관찰보다도 앞서도록 한다.
    permit
        .validate_for(project.fingerprint(), &request.targets)
        .map_err(|source| MigrationRecoveryError {
            stage: MigrationRecoveryStage::PermitValidation,
            project_fingerprint: project.fingerprint().to_owned(),
            target: None,
            source: Some(Box::new(MigrationRecoveryErrorSource::Permit(source))),
        })?;

    Ok(())
}

fn finish_recovery_report(
    project: &LockedProject<'_>,
    request: MigrationRecoveryRequest,
    scope: MigrationRecoveryScope<'_, '_>,
    transaction: RecoveryReport,
) -> MigrationRecoveryReport {
    MigrationRecoveryReport {
        project_fingerprint: project.fingerprint().to_owned(),
        targets: request.targets,
        migration: scope.observed.into_inner(),
        transaction,
    }
}

fn map_recovery_error(
    project: &LockedProject<'_>,
    source: RecoveryError,
) -> MigrationRecoveryError {
    let stage = if source.failure().stage == super::transaction::RecoveryStage::InspectJournal {
        MigrationRecoveryStage::InspectMigrationJournal
    } else {
        MigrationRecoveryStage::TransactionRecovery
    };
    MigrationRecoveryError {
        stage,
        project_fingerprint: project.fingerprint().to_owned(),
        target: source.failure().target.clone(),
        source: Some(Box::new(MigrationRecoveryErrorSource::Transaction(source))),
    }
}

struct MigrationRecoveryScope<'project, 'lock> {
    project: &'project LockedProject<'lock>,
    expected_targets: Vec<ProjectRelativePath>,
    observed: RefCell<Option<RecoveredMigrationTransaction>>,
}

impl<'project, 'lock> MigrationRecoveryScope<'project, 'lock> {
    fn new(
        project: &'project LockedProject<'lock>,
        expected_targets: &[ProjectRelativePath],
    ) -> Self {
        Self {
            project,
            expected_targets: expected_targets.to_vec(),
            observed: RefCell::new(None),
        }
    }

    fn inspect(
        &self,
        transaction_id: &TransactionId,
        directory: &Path,
        manifest: &TransactionManifest,
    ) -> Result<RecoveredMigrationTransaction, &'static str> {
        let targets: Vec<_> = manifest
            .operations
            .iter()
            .map(|operation| operation.target_path.clone())
            .collect();
        if targets != self.expected_targets {
            return Err("migration journal targets differ from the exact recovery permit");
        }

        let transitions = manifest
            .operations
            .iter()
            .map(|operation| {
                if !operation.original_existed {
                    return Err("migration journal contains a newly created target");
                }
                let source = operation
                    .original_schema_version
                    .ok_or("migration original schemaVersion is missing")?;
                let target = operation
                    .staged_schema_version
                    .ok_or("migration staged schemaVersion is missing")?;
                if source >= target {
                    return Err("migration operation is not a strict schemaVersion increase");
                }
                Ok(MigrationVersionTransition { source, target })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let state = read_and_validate_state(directory, transaction_id, self.project)?;
        validate_state_progress(&state, manifest.operations.len())?;
        validate_migration_artifacts(directory, manifest, state.state, self.project)?;
        Ok(RecoveredMigrationTransaction {
            transaction_id: transaction_id.clone(),
            transitions,
        })
    }
}

impl RecoveryScope for MigrationRecoveryScope<'_, '_> {
    fn validate_manifest(
        &self,
        transaction_id: &TransactionId,
        directory: &Path,
        manifest: &TransactionManifest,
    ) -> Result<(), &'static str> {
        let inspected = self.inspect(transaction_id, directory, manifest)?;
        let mut observed = self.observed.borrow_mut();
        match observed.as_ref() {
            None => *observed = Some(inspected),
            Some(existing) if existing == &inspected => {}
            Some(_) => return Err("more than one migration transaction is pending"),
        }
        Ok(())
    }

    fn validate_missing_manifest(
        &self,
        _transaction_id: &TransactionId,
        _directory: &Path,
    ) -> Result<(), &'static str> {
        Err("migration recovery requires a complete prepared manifest")
    }
}

fn read_and_validate_state(
    directory: &Path,
    transaction_id: &TransactionId,
    project: &LockedProject<'_>,
) -> Result<TransactionStateRecord, &'static str> {
    let bytes = read_regular_required(directory, &directory.join("state.json"))?;
    let state: TransactionStateRecord = serde_json::from_slice(&bytes)
        .map_err(|_| "migration transaction state JSON is invalid")?;
    state
        .validate()
        .map_err(|_| "migration transaction state validation failed")?;
    if state.transaction_id != *transaction_id || state.project_fingerprint != project.fingerprint()
    {
        return Err("migration transaction state identity differs");
    }
    Ok(state)
}

fn validate_state_progress(
    state: &TransactionStateRecord,
    operation_count: usize,
) -> Result<(), &'static str> {
    let progress = state.applied_operations.len();
    let valid = match state.state {
        // canonical v2 전용 정리 상태는 migration의 v1 진행 증거가 아니다.
        TransactionState::Preparing
        | TransactionState::CleaningCommitted
        | TransactionState::CleaningRolledBack => false,
        TransactionState::Prepared => progress == 0,
        TransactionState::Applying | TransactionState::RollingBack => progress <= operation_count,
        TransactionState::Committed => progress == operation_count,
        TransactionState::RolledBack => progress == 0,
    };
    if valid {
        Ok(())
    } else {
        Err("migration transaction state progress is inconsistent")
    }
}

fn validate_migration_artifacts(
    directory: &Path,
    manifest: &TransactionManifest,
    state: TransactionState,
    project: &LockedProject<'_>,
) -> Result<(), &'static str> {
    for operation in &manifest.operations {
        let backup_path = operation
            .backup_path
            .as_deref()
            .ok_or("migration backup path is missing")?;
        let backup = read_regular_required(directory, &directory.join(backup_path))?;
        verify_operation_bytes(operation, &backup, false)?;

        match read_regular_optional(directory, &directory.join(&operation.staged_path))? {
            Some(staged) => verify_operation_bytes(operation, &staged, true)?,
            None => {
                let target = read_target(project, &operation.target_path)?;
                let matches_staged = target
                    .as_deref()
                    .is_some_and(|bytes| operation_bytes_match(operation, bytes, true));
                let rollback_can_explain_missing = matches!(
                    state,
                    TransactionState::RollingBack | TransactionState::RolledBack
                ) && target
                    .as_deref()
                    .is_none_or(|bytes| operation_bytes_match(operation, bytes, false));
                if !matches_staged && !rollback_can_explain_missing {
                    return Err("migration staged artifact is missing or invalid");
                }
            }
        }
    }
    Ok(())
}

fn read_regular_required(directory: &Path, path: &Path) -> Result<Vec<u8>, &'static str> {
    read_regular_optional(directory, path)?
        .ok_or("required migration transaction artifact is missing")
}

fn read_regular_optional(directory: &Path, path: &Path) -> Result<Option<Vec<u8>>, &'static str> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("migration transaction artifact metadata cannot be read"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("migration transaction artifact is not a regular file");
    }
    let mut file = open_existing_private_file(directory, path)
        .map_err(|_| "migration transaction artifact cannot be safely opened")?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| "migration transaction artifact cannot be read")?;
    Ok(Some(bytes))
}

fn read_target(
    project: &LockedProject<'_>,
    target: &ProjectRelativePath,
) -> Result<Option<Vec<u8>>, &'static str> {
    let mut file = match open_existing_project_file(project.canonical_root(), target) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("migration target cannot be safely opened"),
    };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|_| "migration target cannot be read")?;
    Ok(Some(bytes))
}

fn verify_operation_bytes(
    operation: &TransactionOperation,
    bytes: &[u8],
    staged: bool,
) -> Result<(), &'static str> {
    if operation_bytes_match(operation, bytes, staged) {
        Ok(())
    } else {
        Err("migration transaction artifact differs from manifest metadata")
    }
}

fn operation_bytes_match(operation: &TransactionOperation, bytes: &[u8], staged: bool) -> bool {
    let (expected_size, expected_hash, expected_schema) = if staged {
        (
            Some(operation.staged_size),
            Some(operation.staged_sha256.as_str()),
            operation.staged_schema_version,
        )
    } else {
        (
            operation.original_size,
            operation.original_sha256.as_deref(),
            operation.original_schema_version,
        )
    };
    expected_size == Some(bytes.len() as u64)
        && expected_hash == Some(sha256(bytes).as_str())
        && strict_schema_version(bytes).ok() == expected_schema
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(digest.len() * 2);
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for byte in digest {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use serde_json::{json, Value};

    use super::*;
    use crate::data::json::to_deterministic_json_bytes;
    use crate::data::migration::{MigrationRegistry, MigrationStep, MigrationStepError};
    use crate::data::migration_prepare::{
        prepare_migration, MigrationPrepareRequest, PreparedMigrationTransaction,
    };
    use crate::data::project_lock::ProjectLock;
    use crate::data::transaction::{
        recover_pending_transactions, test_write_permit, CommitResultState, TransactionPlan,
    };

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
    type TestResult = Result<(), Box<dyn Error>>;

    struct Temp(PathBuf);

    impl Temp {
        fn new() -> io::Result<Self> {
            for _ in 0..128 {
                let path = std::env::temp_dir().join(format!(
                    "worldbuild-migration-recovery-{}-{}",
                    std::process::id(),
                    TEST_COUNTER.fetch_add(1, Ordering::Relaxed)
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

    impl Drop for Temp {
        fn drop(&mut self) {
            let _cleanup = fs::remove_dir_all(&self.0);
        }
    }

    fn with_project<F>(test: F) -> TestResult
    where
        F: FnOnce(&Path, &LockedProject<'_>) -> TestResult,
    {
        let temp = Temp::new()?;
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
        Ok(value)
    }
    fn v2_to_v3(mut value: Value) -> Result<Value, MigrationStepError> {
        value["schemaVersion"] = json!(3);
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

    fn prepare<'project, 'lock>(
        root: &Path,
        project: &'project LockedProject<'lock>,
        sources: &[(&str, u32)],
    ) -> Result<PreparedMigrationTransaction<'project, 'lock, 'static>, Box<dyn Error>> {
        let targets = sources
            .iter()
            .map(|(path, _)| target(path))
            .collect::<Vec<_>>();
        for (path, version) in sources {
            fs::write(
                root.join(path),
                format!(r#"{{"schemaVersion":{version},"name":"{path}"}}"#),
            )?;
        }
        let permit = test_write_permit(project.fingerprint(), targets.clone());
        Ok(prepare_migration(
            MigrationPrepareRequest {
                targets,
                registry: registry(),
            },
            project,
            permit,
        )?)
    }

    fn only_transaction_directory(project: &LockedProject<'_>) -> io::Result<PathBuf> {
        let mut entries = fs::read_dir(project.transactions_root())?;
        let directory = entries
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "transaction missing"))??
            .path();
        if entries.next().is_some() {
            return Err(io::Error::other("more than one transaction"));
        }
        Ok(directory)
    }

    #[test]
    fn prepared_mixed_migration_recovers_without_registry_and_is_idempotent() -> TestResult {
        with_project(|root, project| {
            let sources = [("data/a.json", 1), ("data/b.json", 2), ("data/c.json", 1)];
            let prepared = prepare(root, project, &sources)?;
            let originals = sources
                .iter()
                .map(|(path, _)| (path.to_string(), fs::read(root.join(path))))
                .map(|(path, bytes)| bytes.map(|bytes| (path, bytes)))
                .collect::<Result<Vec<_>, _>>()?;
            let targets = prepared.targets().to_vec();
            let transaction_id = prepared.transaction_id().clone();
            drop(prepared);

            let report = recover_migration(
                MigrationRecoveryRequest {
                    targets: targets.clone(),
                },
                project,
                test_write_permit(project.fingerprint(), targets.clone()),
            )?;
            assert_eq!(report.targets, targets);
            assert_eq!(report.project_fingerprint, project.fingerprint());
            let migration = report.migration.ok_or("migration identity missing")?;
            assert_eq!(migration.transaction_id, transaction_id);
            assert_eq!(
                migration
                    .transitions
                    .iter()
                    .map(|transition| (transition.source.get(), transition.target.get()))
                    .collect::<Vec<_>>(),
                [(1, 3), (2, 3), (1, 3)]
            );
            assert_eq!(report.transaction.rolled_back_transactions, 1);
            for (path, bytes) in originals {
                assert_eq!(fs::read(root.join(path))?, bytes);
            }

            let second = recover_migration(
                MigrationRecoveryRequest {
                    targets: targets.clone(),
                },
                project,
                test_write_permit(project.fingerprint(), targets),
            )?;
            assert!(second.transaction.nothing_to_recover());
            assert!(second.migration.is_none());
            Ok(())
        })
    }

    #[test]
    fn rejects_ordinary_transaction_then_generic_recovery_still_works() -> TestResult {
        with_project(|root, project| {
            let target = target("data/a.json");
            let original = to_deterministic_json_bytes(&json!({
                "schemaVersion": 1,
                "name": "old"
            }))?;
            fs::write(root.join(target.as_str()), &original)?;
            let mut plan = TransactionPlan::new();
            plan.add_json(
                target.as_str(),
                &json!({"schemaVersion": 1, "name": "ordinary"}),
            )?;
            let prepared = plan.prepare(
                project,
                test_write_permit(project.fingerprint(), vec![target.clone()]),
            )?;
            drop(prepared);

            let error = recover_migration(
                MigrationRecoveryRequest {
                    targets: vec![target.clone()],
                },
                project,
                test_write_permit(project.fingerprint(), vec![target.clone()]),
            )
            .expect_err("ordinary transaction must not be migration recovery");
            assert_eq!(error.stage, MigrationRecoveryStage::InspectMigrationJournal);
            assert_eq!(fs::read(root.join(target.as_str()))?, original);
            assert!(only_transaction_directory(project)?.is_dir());

            let generic = recover_pending_transactions(project)?;
            assert_eq!(generic.rolled_back_transactions, 1);
            assert_eq!(fs::read(root.join(target.as_str()))?, original);
            Ok(())
        })
    }

    #[test]
    fn permit_mismatch_fails_before_transaction_filesystem_access() -> TestResult {
        with_project(|root, project| {
            let authorized = target("data/a.json");
            let requested = target("data/b.json");
            let error = recover_migration(
                MigrationRecoveryRequest {
                    targets: vec![requested],
                },
                project,
                test_write_permit(project.fingerprint(), vec![authorized]),
            )
            .expect_err("mismatched permit must fail");
            assert_eq!(error.stage, MigrationRecoveryStage::PermitValidation);
            let diagnostic = format!("{error:?}\n{error}");
            assert!(!diagnostic.contains(project.fingerprint()));
            let source = error.source().ok_or("redacted source category missing")?;
            assert!(!format!("{source:?}\n{source}").contains(project.fingerprint()));
            assert!(source.source().is_none());
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })
    }

    #[test]
    fn corrupt_manifest_state_staged_or_backup_never_changes_targets() -> TestResult {
        for artifact in ["manifest.json", "state.json", "staged", "backup"] {
            with_project(|root, project| {
                let prepared = prepare(root, project, &[("data/a.json", 1)])?;
                let target = prepared.targets()[0].clone();
                let source = fs::read(root.join(target.as_str()))?;
                let directory = only_transaction_directory(project)?;
                let manifest: TransactionManifest =
                    serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
                let operation = manifest.operations[0].clone();
                drop(prepared);
                let path = match artifact {
                    "manifest.json" => directory.join("manifest.json"),
                    "state.json" => directory.join("state.json"),
                    "staged" => directory.join(operation.staged_path),
                    "backup" => {
                        directory.join(operation.backup_path.as_deref().ok_or("backup missing")?)
                    }
                    _ => unreachable!(),
                };
                fs::write(path, b"corrupt")?;

                let error = recover_migration(
                    MigrationRecoveryRequest {
                        targets: vec![target.clone()],
                    },
                    project,
                    test_write_permit(project.fingerprint(), vec![target.clone()]),
                )
                .expect_err("corrupt migration journal must be rejected");
                let diagnostic = format!("{error:?}\n{error}");
                assert!(!diagnostic.contains(&root.display().to_string()));
                assert!(!diagnostic.contains(project.fingerprint()));
                assert!(!diagnostic.contains("schemaVersion"));
                assert!(!diagnostic.contains(&operation.staged_sha256));
                let mut current_source = error.source();
                while let Some(value) = current_source {
                    let source_diagnostic = format!("{value:?}\n{value}");
                    assert!(!source_diagnostic.contains(project.fingerprint()));
                    assert!(!source_diagnostic.contains(&root.display().to_string()));
                    assert!(!source_diagnostic.contains(&operation.staged_sha256));
                    current_source = value.source();
                }
                assert_eq!(fs::read(root.join(target.as_str()))?, source, "{artifact}");
                assert!(directory.is_dir(), "{artifact}");
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn migration_commit_success_report_keeps_transaction_outcome() -> TestResult {
        with_project(|root, project| {
            let prepared = prepare(root, project, &[("data/a.json", 1)])?;
            let report = prepared.commit();
            assert_eq!(report.outcome.result_state(), CommitResultState::Committed);
            assert_eq!(
                strict_schema_version(&fs::read(root.join("data/a.json"))?)?.get(),
                3
            );
            Ok(())
        })
    }
}
