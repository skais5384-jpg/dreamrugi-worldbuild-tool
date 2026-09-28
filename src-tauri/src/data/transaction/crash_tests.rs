use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};

use super::super::collaboration_lock::{
    LockCoordinator, LockService, LockSessionId, NoLockService,
};
use super::super::json::to_deterministic_json_bytes;
use super::super::migration::{MigrationRegistry, MigrationStep, MigrationStepError};
use super::super::migration_prepare::{prepare_migration, MigrationPrepareRequest};
use super::super::migration_recovery::{recover_migration, MigrationRecoveryRequest};
use super::super::project_lock::ProjectLock;
use super::apply::{commit_with_hooks, CommitFailPoint, CommitHooks};
use super::prepare::{prepare_with_hooks_for_test, sha256, PrepareFailPoint, PrepareHooks};
use super::recovery::{recover_pending_transactions_with_hooks, RecoveryFailPoint, RecoveryHooks};
use super::*;

static COUNTER: AtomicU64 = AtomicU64::new(0);
type TestResult = Result<(), Box<dyn std::error::Error>>;
const CHILD_TIMEOUT: Duration = Duration::from_secs(15);
const LOCK_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Document<'a> {
    schema_version: u32,
    title: &'a str,
}

struct Temp(PathBuf);

impl Temp {
    fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let path = std::env::temp_dir().join(format!(
                "worldbuild-crash-matrix-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CrashPoint {
    DirectoryAllocated,
    PreparingState,
    PartialStaged,
    PartialBackup,
    ManifestBeforePrepared,
    PreparedState,
    ApplyingState,
    AppliedFirst,
    AppliedMiddle,
    AppliedLastBeforeMarker,
    MarkerPostReplaceBeforeSync,
    CommittedMarker,
    CommittedState,
    RollingBackState,
    RollbackFirst,
    RollbackMiddle,
    RolledBackMarker,
}

impl CrashPoint {
    const ALL: [Self; 16] = [
        Self::DirectoryAllocated,
        Self::PreparingState,
        Self::PartialStaged,
        Self::PartialBackup,
        Self::ManifestBeforePrepared,
        Self::PreparedState,
        Self::ApplyingState,
        Self::AppliedFirst,
        Self::AppliedMiddle,
        Self::AppliedLastBeforeMarker,
        Self::CommittedMarker,
        Self::CommittedState,
        Self::RollingBackState,
        Self::RollbackFirst,
        Self::RollbackMiddle,
        Self::RolledBackMarker,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::DirectoryAllocated => "directory-allocated",
            Self::PreparingState => "preparing-state",
            Self::PartialStaged => "partial-staged",
            Self::PartialBackup => "partial-backup",
            Self::ManifestBeforePrepared => "manifest-before-prepared",
            Self::PreparedState => "prepared-state",
            Self::ApplyingState => "applying-state",
            Self::AppliedFirst => "applied-first",
            Self::AppliedMiddle => "applied-middle",
            Self::AppliedLastBeforeMarker => "applied-last-before-marker",
            Self::MarkerPostReplaceBeforeSync => "committed-marker-after-replace",
            Self::CommittedMarker => "committed-marker",
            Self::CommittedState => "committed-state",
            Self::RollingBackState => "rolling-back-state",
            Self::RollbackFirst => "rollback-first",
            Self::RollbackMiddle => "rollback-middle",
            Self::RolledBackMarker => "rolled-back-marker",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .chain([Self::MarkerPostReplaceBeforeSync])
            .find(|point| point.name() == value)
    }

    fn expects_committed(self) -> bool {
        matches!(
            self,
            Self::MarkerPostReplaceBeforeSync | Self::CommittedMarker | Self::CommittedState
        )
    }

    fn is_rollback(self) -> bool {
        matches!(
            self,
            Self::RollingBackState
                | Self::RollbackFirst
                | Self::RollbackMiddle
                | Self::RolledBackMarker
        )
    }

    fn has_manifest(self) -> bool {
        !matches!(
            self,
            Self::DirectoryAllocated
                | Self::PreparingState
                | Self::PartialStaged
                | Self::PartialBackup
        )
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _kill = self.0.kill();
        let _wait = self.0.wait();
    }
}

struct PauseHook<'a> {
    point: CrashPoint,
    ready: &'a Path,
}

impl PauseHook<'_> {
    fn pause(&self) -> ! {
        publish_ready(self.ready, self.point.name()).expect("write crash ready signal");
        loop {
            thread::park_timeout(Duration::from_secs(1));
        }
    }
}

/// parent가 생성 직후의 빈 ready 파일을 관측하지 않도록 완성된 payload만 공개한다.
fn publish_ready(ready: &Path, payload: &str) -> io::Result<()> {
    let temporary = ready.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(payload.as_bytes())?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, ready)
}

impl PrepareHooks for PauseHook<'_> {
    fn check(&self, point: PrepareFailPoint, operation: Option<u32>) -> io::Result<()> {
        let selected = match self.point {
            CrashPoint::DirectoryAllocated => {
                point == PrepareFailPoint::TransactionDirectoryAllocated
            }
            CrashPoint::PreparingState => {
                point == PrepareFailPoint::StagedCreate && operation == Some(0)
            }
            CrashPoint::PartialStaged => {
                point == PrepareFailPoint::StagedCreate && operation == Some(1)
            }
            CrashPoint::PartialBackup => {
                point == PrepareFailPoint::BackupCreate && operation == Some(2)
            }
            CrashPoint::ManifestBeforePrepared => point == PrepareFailPoint::PreparedState,
            _ => false,
        };
        if selected {
            self.pause();
        }
        Ok(())
    }
}

impl CommitHooks for PauseHook<'_> {
    fn check(&self, point: CommitFailPoint, operation: Option<u32>) -> io::Result<()> {
        let selected = match self.point {
            CrashPoint::PreparedState => point == CommitFailPoint::ManifestRevalidation,
            CrashPoint::ApplyingState => {
                point == CommitFailPoint::TargetPrecondition && operation == Some(0)
            }
            CrashPoint::AppliedFirst => {
                point == CommitFailPoint::TargetPrecondition && operation == Some(1)
            }
            CrashPoint::AppliedMiddle => {
                point == CommitFailPoint::TargetPrecondition && operation == Some(2)
            }
            CrashPoint::AppliedLastBeforeMarker => point == CommitFailPoint::CommittedMarkerWrite,
            CrashPoint::MarkerPostReplaceBeforeSync => {
                point == CommitFailPoint::CommittedMarkerAfterReplace
            }
            CrashPoint::CommittedMarker => point == CommitFailPoint::CommittedState,
            CrashPoint::CommittedState => point == CommitFailPoint::Cleanup,
            _ => false,
        };
        if selected {
            self.pause();
        }
        Ok(())
    }
}

impl RecoveryHooks for PauseHook<'_> {
    fn check(&self, point: RecoveryFailPoint, operation: Option<u32>) -> io::Result<()> {
        let selected = match self.point {
            CrashPoint::RollingBackState => {
                point == RecoveryFailPoint::BackupVerify && operation == Some(2)
            }
            CrashPoint::RollbackFirst => {
                point == RecoveryFailPoint::ProgressState && operation == Some(1)
            }
            CrashPoint::RollbackMiddle => {
                point == RecoveryFailPoint::ProgressState && operation == Some(0)
            }
            CrashPoint::RolledBackMarker => point == RecoveryFailPoint::RolledBackState,
            _ => false,
        };
        if selected {
            self.pause();
        }
        Ok(())
    }
}

#[test]
fn forced_exit_crash_matrix_recovers_old_or_committed_state() -> TestResult {
    for point in CrashPoint::ALL {
        run_case(point)?;
    }
    Ok(())
}

#[test]
fn forced_exit_partial_backup_recovers_old_state() -> TestResult {
    run_case(CrashPoint::PartialBackup)
}

#[test]
fn forced_exit_after_committed_marker_replace_rolls_forward() -> TestResult {
    run_case(CrashPoint::MarkerPostReplaceBeforeSync)
}

#[test]
fn migration_commit_and_recovery_crash_matrix_is_all_original_or_all_migrated() -> TestResult {
    for point in [
        CrashPoint::PreparedState,
        CrashPoint::ApplyingState,
        CrashPoint::AppliedFirst,
        CrashPoint::AppliedMiddle,
        CrashPoint::AppliedLastBeforeMarker,
        CrashPoint::CommittedMarker,
        CrashPoint::CommittedState,
        CrashPoint::RollingBackState,
        CrashPoint::RollbackFirst,
        CrashPoint::RollbackMiddle,
        CrashPoint::RolledBackMarker,
    ] {
        run_migration_case(point)?;
    }
    Ok(())
}

fn run_migration_case(point: CrashPoint) -> TestResult {
    let temp = Temp::new()?;
    let root = temp.0.join("project");
    let lock_root = temp.0.join("app-data");
    let ready = temp.0.join("ready");
    fs::create_dir(&root)?;
    fs::create_dir(root.join("data"))?;
    write_migration_sources(&root)?;

    if point.is_rollback() {
        run_migration_child(
            &root,
            &lock_root,
            &ready,
            CrashPoint::AppliedLastBeforeMarker,
            "commit",
        )?;
        fs::remove_file(&ready)?;
    }
    run_migration_child(
        &root,
        &lock_root,
        &ready,
        point,
        if point.is_rollback() {
            "recover"
        } else {
            "commit"
        },
    )?;

    let transaction_directory = only_transaction_directory(&root)?;
    assert_migration_crash_evidence(&root, &transaction_directory)?;
    let lock = retry_lock(&root, &lock_root, LOCK_TIMEOUT)?;
    let project = LockedProject::bind(&lock, &root)?;
    let targets = migration_targets();
    let report = recover_migration(
        MigrationRecoveryRequest {
            targets: targets.clone(),
        },
        &project,
        test_write_permit(project.fingerprint(), targets.clone()),
    )?;
    assert_eq!(report.transaction.discovered_transactions, 1, "{point:?}");
    assert_migration_targets(&root, point.expects_committed())?;
    assert!(!transaction_directory.exists(), "{point:?}");

    let repeated = recover_migration(
        MigrationRecoveryRequest {
            targets: targets.clone(),
        },
        &project,
        test_write_permit(project.fingerprint(), targets),
    )?;
    assert!(repeated.transaction.nothing_to_recover(), "{point:?}");
    assert_migration_targets(&root, point.expects_committed())?;
    Ok(())
}

fn run_migration_child(
    root: &Path,
    lock_root: &Path,
    ready: &Path,
    point: CrashPoint,
    action: &str,
) -> TestResult {
    let child = Command::new(std::env::current_exe()?)
        .arg("--ignored")
        .arg("--exact")
        .arg("data::transaction::crash_tests::crash_matrix_child_helper")
        .arg("--nocapture")
        .env("WORLDBUILD_MIGRATION_CRASH_PROJECT", root)
        .env("WORLDBUILD_CRASH_LOCK_ROOT", lock_root)
        .env("WORLDBUILD_CRASH_READY", ready)
        .env("WORLDBUILD_CRASH_POINT", point.name())
        .env("WORLDBUILD_MIGRATION_CRASH_ACTION", action)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child = ChildGuard(child);
    wait_for_ready(&mut child.0, ready, point)?;
    child.0.kill()?;
    child.0.wait()?;
    Ok(())
}

fn run_case(point: CrashPoint) -> TestResult {
    let temp = Temp::new()?;
    let root = temp.0.join("project");
    let lock_root = temp.0.join("app-data");
    let ready = temp.0.join("ready");
    fs::create_dir(&root)?;
    fs::create_dir(root.join("data"))?;
    fs::write(root.join("data/a.json"), bytes("old-a")?)?;
    fs::write(root.join("data/c.json"), bytes("old-c")?)?;

    let child = Command::new(std::env::current_exe()?)
        .arg("--ignored")
        .arg("--exact")
        .arg("data::transaction::crash_tests::crash_matrix_child_helper")
        .arg("--nocapture")
        .env("WORLDBUILD_CRASH_PROJECT", &root)
        .env("WORLDBUILD_CRASH_LOCK_ROOT", &lock_root)
        .env("WORLDBUILD_CRASH_READY", &ready)
        .env("WORLDBUILD_CRASH_POINT", point.name())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child = ChildGuard(child);
    wait_for_ready(&mut child.0, &ready, point)?;
    child.0.kill()?;
    child.0.wait()?;

    let transaction_directory = only_transaction_directory(&root)?;
    assert_crash_evidence(point, &transaction_directory)?;

    let lock = retry_lock(&root, &lock_root, LOCK_TIMEOUT)?;
    let project = LockedProject::bind(&lock, &root)?;
    let report = recover_pending_transactions(&project)?;
    assert_eq!(report.discovered_transactions, 1, "{point:?}");
    assert!(!report.manual_recovery_required(), "{point:?}");
    assert_final_targets(&root, point.expects_committed())?;
    assert!(!transaction_directory.exists(), "{point:?}");
    assert!(project.transactions_root().is_dir(), "{point:?}");
    assert!(fs::read_dir(project.transactions_root())?.next().is_none());
    Ok(())
}

fn assert_crash_evidence(point: CrashPoint, directory: &Path) -> TestResult {
    assert!(directory.is_dir(), "{point:?}");
    let manifest_path = directory.join("manifest.json");
    assert_eq!(manifest_path.is_file(), point.has_manifest(), "{point:?}");
    if !point.has_manifest() {
        assert!(!directory.join("committed.json").exists(), "{point:?}");
        assert!(!directory.join("rolled-back.json").exists(), "{point:?}");
        return Ok(());
    }

    let manifest: TransactionManifest = serde_json::from_slice(&fs::read(manifest_path)?)?;
    manifest.validate()?;
    assert_eq!(
        manifest
            .operations
            .iter()
            .map(|operation| operation.target_path.as_str())
            .collect::<Vec<_>>(),
        ["data/a.json", "data/b.json", "data/c.json"]
    );
    for operation in &manifest.operations {
        assert!(!operation.staged_sha256.is_empty(), "{point:?}");
        assert!(!operation.original_existed || operation.original_sha256.is_some());
        if operation.original_existed {
            assert!(
                directory
                    .join(
                        operation
                            .backup_path
                            .as_deref()
                            .ok_or("backup path missing")?
                    )
                    .is_file(),
                "{point:?}"
            );
        }
        if !point.is_rollback() {
            let staged = directory.join(&operation.staged_path);
            if !staged.exists() {
                let target = directory
                    .parent()
                    .and_then(Path::parent)
                    .and_then(Path::parent)
                    .ok_or("project root missing")?
                    .join(operation.target_path.as_str());
                let target_bytes = fs::read(target)?;
                assert_eq!(
                    target_bytes.len() as u64,
                    operation.staged_size,
                    "{point:?}"
                );
                assert_eq!(sha256(&target_bytes), operation.staged_sha256, "{point:?}");
            }
        }
    }
    assert_eq!(
        directory.join("committed.json").is_file(),
        point.expects_committed(),
        "{point:?}"
    );
    assert_eq!(
        directory.join("rolled-back.json").is_file(),
        point == CrashPoint::RolledBackMarker,
        "{point:?}"
    );
    assert!(
        !(directory.join("committed.json").exists() && directory.join("rolled-back.json").exists()),
        "{point:?}"
    );
    Ok(())
}

fn assert_final_targets(root: &Path, committed: bool) -> TestResult {
    if committed {
        assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new-a")?);
        assert_eq!(fs::read(root.join("data/b.json"))?, bytes("new-b")?);
        assert_eq!(fs::read(root.join("data/c.json"))?, bytes("new-c")?);
    } else {
        assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
        assert!(!root.join("data/b.json").exists());
        assert_eq!(fs::read(root.join("data/c.json"))?, bytes("old-c")?);
    }
    Ok(())
}

#[test]
#[ignore = "helper launched by forced_exit_crash_matrix_recovers_old_or_committed_state"]
fn crash_matrix_child_helper() -> TestResult {
    if std::env::var_os("WORLDBUILD_MIGRATION_CRASH_PROJECT").is_some() {
        return migration_crash_child_helper();
    }
    let Some(root) = std::env::var_os("WORLDBUILD_CRASH_PROJECT") else {
        return Ok(());
    };
    let lock_root = std::env::var_os("WORLDBUILD_CRASH_LOCK_ROOT").ok_or("lock root missing")?;
    let ready = std::env::var_os("WORLDBUILD_CRASH_READY").ok_or("ready path missing")?;
    let point = std::env::var("WORLDBUILD_CRASH_POINT")
        .ok()
        .and_then(|value| CrashPoint::parse(&value))
        .ok_or("invalid crash point")?;
    let root = Path::new(&root);
    let lock = ProjectLock::try_acquire(root, Path::new(&lock_root))?;
    let project = LockedProject::bind(&lock, root)?;
    let hook = PauseHook {
        point,
        ready: Path::new(&ready),
    };

    let plan = mixed_plan()?;
    if matches!(
        point,
        CrashPoint::DirectoryAllocated
            | CrashPoint::PreparingState
            | CrashPoint::PartialStaged
            | CrashPoint::PartialBackup
            | CrashPoint::ManifestBeforePrepared
    ) {
        let _prepared = prepare_with_hooks_for_test(plan, &project, &hook)?;
        return Err("prepare crash point was not reached".into());
    }

    let prepared = plan.prepare_for_test(&project)?;
    if point.is_rollback() {
        apply_all(&prepared)?;
        drop(prepared);
        let _report = recover_pending_transactions_with_hooks(&project, &hook)?;
        return Err("rollback crash point was not reached".into());
    }
    let _outcome = commit_with_hooks(prepared, &hook)?;
    Err("commit crash point was not reached".into())
}

fn migration_crash_child_helper() -> TestResult {
    let root = std::env::var_os("WORLDBUILD_MIGRATION_CRASH_PROJECT")
        .ok_or("migration project root missing")?;
    let lock_root = std::env::var_os("WORLDBUILD_CRASH_LOCK_ROOT").ok_or("lock root missing")?;
    let _ready = std::env::var_os("WORLDBUILD_CRASH_READY").ok_or("ready path missing")?;
    let action = std::env::var("WORLDBUILD_MIGRATION_CRASH_ACTION")?;
    let _point = std::env::var("WORLDBUILD_CRASH_POINT")
        .ok()
        .and_then(|value| CrashPoint::parse(&value))
        .ok_or("invalid migration crash point")?;
    let root = Path::new(&root);
    let lock = retry_lock(root, Path::new(&lock_root), LOCK_TIMEOUT)?;
    let project = LockedProject::bind(&lock, root)?;
    let targets = migration_targets();

    if action == "commit" {
        let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
        let coordinator = LockCoordinator::new(service);
        let session = LockSessionId::generate()?;
        let mut guard =
            coordinator.acquire_all(project.fingerprint(), &session, targets.clone())?;
        let permit = guard.write_permit()?;
        let prepared = prepare_migration(
            MigrationPrepareRequest {
                targets: targets.clone(),
                registry: migration_registry(),
            },
            &project,
            permit,
        )?;
        let _report = prepared.commit();
        return Err("migration commit crash point was not reached".into());
    }
    if action == "recover" {
        let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
        let coordinator = LockCoordinator::new(service);
        let session = LockSessionId::generate()?;
        let mut guard =
            coordinator.acquire_all(project.fingerprint(), &session, targets.clone())?;
        let permit = guard.write_permit()?;
        let _report = recover_migration(
            MigrationRecoveryRequest {
                targets: targets.clone(),
            },
            &project,
            permit,
        )?;
        return Err("migration recovery crash point was not reached".into());
    }
    Err("invalid migration crash action".into())
}

fn validate_migration_version(value: &Value, expected: u64) -> Result<(), MigrationStepError> {
    if value.get("schemaVersion").and_then(Value::as_u64) == Some(expected) {
        Ok(())
    } else {
        Err(MigrationStepError::new(
            "unexpected crash migration version",
        ))
    }
}

fn validate_migration_v1(value: &Value) -> Result<(), MigrationStepError> {
    validate_migration_version(value, 1)
}

fn validate_migration_v2(value: &Value) -> Result<(), MigrationStepError> {
    validate_migration_version(value, 2)
}

fn validate_migration_v3(value: &Value) -> Result<(), MigrationStepError> {
    validate_migration_version(value, 3)
}

fn crash_migrate_v1_v2(mut value: Value) -> Result<Value, MigrationStepError> {
    value["schemaVersion"] = json!(2);
    value["v2"] = json!(true);
    Ok(value)
}

fn crash_migrate_v2_v3(mut value: Value) -> Result<Value, MigrationStepError> {
    value["schemaVersion"] = json!(3);
    value["v3"] = json!(true);
    Ok(value)
}

static MIGRATION_STEPS: [MigrationStep; 2] = [
    MigrationStep::new(
        1,
        2,
        crash_migrate_v1_v2,
        validate_migration_v1,
        validate_migration_v2,
    ),
    MigrationStep::new(
        2,
        3,
        crash_migrate_v2_v3,
        validate_migration_v2,
        validate_migration_v3,
    ),
];

fn migration_registry() -> MigrationRegistry<'static> {
    MigrationRegistry::try_new(3, &MIGRATION_STEPS).expect("crash migration registry is valid")
}

fn migration_targets() -> Vec<ProjectRelativePath> {
    ["data/a.json", "data/b.json", "data/c.json"]
        .into_iter()
        .map(|path| ProjectRelativePath::parse(path).expect("crash migration target is valid"))
        .collect()
}

fn migration_source(version: u32, title: &str) -> Result<Vec<u8>, serde_json::Error> {
    to_deterministic_json_bytes(&json!({"schemaVersion": version, "title": title}))
}

fn migration_expected(version: u32, title: &str) -> Result<Vec<u8>, serde_json::Error> {
    let mut value = json!({"schemaVersion": 3, "title": title, "v3": true});
    if version == 1 {
        value["v2"] = json!(true);
    }
    to_deterministic_json_bytes(&value)
}

fn write_migration_sources(root: &Path) -> TestResult {
    for (path, version, title) in [
        ("data/a.json", 1, "old-a"),
        ("data/b.json", 2, "old-b"),
        ("data/c.json", 1, "old-c"),
    ] {
        fs::write(root.join(path), migration_source(version, title)?)?;
    }
    Ok(())
}

fn assert_migration_targets(root: &Path, migrated: bool) -> TestResult {
    for (path, version, title) in [
        ("data/a.json", 1, "old-a"),
        ("data/b.json", 2, "old-b"),
        ("data/c.json", 1, "old-c"),
    ] {
        let expected = if migrated {
            migration_expected(version, title)?
        } else {
            migration_source(version, title)?
        };
        assert_eq!(fs::read(root.join(path))?, expected);
    }
    Ok(())
}

fn assert_migration_crash_evidence(root: &Path, directory: &Path) -> TestResult {
    let manifest: TransactionManifest =
        serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    manifest.validate()?;
    assert_eq!(manifest.operations.len(), 3);
    let expected_sources = [1, 2, 1];
    for (operation, source) in manifest.operations.iter().zip(expected_sources) {
        assert!(operation.original_existed);
        assert_eq!(
            operation.original_schema_version.map(|value| value.get()),
            Some(source)
        );
        assert_eq!(
            operation.staged_schema_version.map(|value| value.get()),
            Some(3)
        );
        let backup = fs::read(
            directory.join(
                operation
                    .backup_path
                    .as_deref()
                    .ok_or("migration crash backup path missing")?,
            ),
        )?;
        assert_eq!(operation.original_size, Some(backup.len() as u64));
        let backup_hash = sha256(&backup);
        assert_eq!(
            operation.original_sha256.as_deref(),
            Some(backup_hash.as_str())
        );
        assert_eq!(
            super::prepare::managed_schema_version(&backup)?.get(),
            source
        );
        let staged_path = directory.join(&operation.staged_path);
        if staged_path.is_file() {
            let staged = fs::read(staged_path)?;
            assert_eq!(sha256(&staged), operation.staged_sha256);
            assert_eq!(staged.len() as u64, operation.staged_size);
            assert_eq!(super::prepare::managed_schema_version(&staged)?.get(), 3);
        } else {
            let target = fs::read(root.join(operation.target_path.as_str()))?;
            let target_hash = sha256(&target);
            assert!(
                target_hash == operation.staged_sha256
                    || operation.original_sha256.as_deref() == Some(target_hash.as_str())
            );
            let expected_schema = if target_hash == operation.staged_sha256 {
                3
            } else {
                source
            };
            assert_eq!(
                super::prepare::managed_schema_version(&target)?.get(),
                expected_schema
            );
        }
    }
    let state: TransactionStateRecord =
        serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
    state.validate()?;
    assert_eq!(state.transaction_id, manifest.transaction_id);
    assert_eq!(state.project_fingerprint, manifest.project_fingerprint);
    Ok(())
}

fn mixed_plan() -> Result<TransactionPlan, TransactionPrepareError> {
    let mut plan = TransactionPlan::new();
    // 입력은 c, b, a 순서지만 manifest와 apply는 target 사전순 a, b, c가 된다.
    for (path, title) in [
        ("data/c.json", "new-c"),
        ("data/b.json", "new-b"),
        ("data/a.json", "new-a"),
    ] {
        plan.add_json(
            path,
            &Document {
                schema_version: 1,
                title,
            },
        )?;
    }
    Ok(plan)
}

fn apply_all(prepared: &PreparedTransaction<'_, '_, '_>) -> io::Result<()> {
    for operation in &prepared.manifest().operations {
        fs::rename(
            prepared
                .transaction_directory()
                .join(&operation.staged_path),
            prepared
                .project
                .canonical_root()
                .join(operation.target_path.as_str()),
        )?;
    }
    Ok(())
}

fn bytes(title: &str) -> Result<Vec<u8>, serde_json::Error> {
    to_deterministic_json_bytes(&Document {
        schema_version: 1,
        title,
    })
}

fn only_transaction_directory(root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(root.join(".worldbuild/transactions"))?;
    let directory = entries
        .next()
        .ok_or("transaction directory missing")??
        .path();
    if entries.next().is_some() {
        return Err("more than one transaction directory found".into());
    }
    Ok(directory)
}

fn wait_for_ready(child: &mut Child, ready: &Path, point: CrashPoint) -> TestResult {
    let deadline = Instant::now() + CHILD_TIMEOUT;
    while Instant::now() < deadline {
        if ready.exists() {
            assert_eq!(fs::read_to_string(ready)?, point.name());
            assert!(
                child.try_wait()?.is_none(),
                "child exited after ready: {point:?}"
            );
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!("child exited before {point:?}: {status}").into());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(format!("timed out waiting for {point:?}").into())
}

fn retry_lock(
    project: &Path,
    lock_root: &Path,
    timeout: Duration,
) -> Result<ProjectLock, Box<dyn std::error::Error>> {
    let deadline = Instant::now() + timeout;
    loop {
        match ProjectLock::try_acquire(project, lock_root) {
            Ok(lock) => return Ok(lock),
            Err(error) if Instant::now() < deadline => {
                let _diagnostic = error;
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => return Err(error.into()),
        }
    }
}
