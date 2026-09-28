use std::cell::Cell;
use std::error::Error;
use std::fs;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;
use sha2::{Digest, Sha256};

use super::super::project_lock::ProjectLockError;
use super::super::project_relative_path::ProjectRelativePath;
use super::super::transaction::{
    self, CommitResultState, RecoveryResultState, RecoveryStage, TransactionId,
    TransactionManifest, TransactionPlan, TransactionState, TransactionStateRecord,
};
use super::*;
use transaction::test_support::{with_commit_failures, CommitTestPoint, RecoveryTestPoint};

type TestResult = Result<(), Box<dyn Error>>;
static COUNTER: AtomicU64 = AtomicU64::new(0);
const CANARY: &str = "C:/Users/private-user/raw-journal-body?credential=secret";

thread_local! {
    static RELEASE_FAULT: Cell<bool> = const { Cell::new(false) };
    static PHASE_OBSERVATIONS: Cell<usize> = const { Cell::new(0) };
    static CREATE_CLEANUP_FAULT: Cell<Option<CreateCleanupFault>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreateCleanupFault {
    Io,
    ExternalEntry,
}

pub(super) fn remove_created_directory(path: &Path) -> io::Result<()> {
    match CREATE_CLEANUP_FAULT.with(Cell::take) {
        Some(CreateCleanupFault::Io) => {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, CANARY))
        }
        Some(CreateCleanupFault::ExternalEntry) => {
            fs::write(path.join("external.txt"), CANARY)?;
            fs::remove_dir(path)
        }
        None => fs::remove_dir(path),
    }
}

fn with_create_cleanup_fault<T>(fault: CreateCleanupFault, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<CreateCleanupFault>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CREATE_CLEANUP_FAULT.with(|slot| slot.set(self.0));
        }
    }
    let _reset = Reset(CREATE_CLEANUP_FAULT.with(|slot| slot.replace(Some(fault))));
    run()
}

pub(super) fn observe_recovering(runtime: &mut ProjectRuntime) {
    // 복구 결과 이후의 flag가 아니라 실제 M1 진입 직전 phase에서 차단을 확인한다.
    assert_eq!(runtime.snapshot().state, RuntimeState::Recovering);
    assert_denied(runtime);
    PHASE_OBSERVATIONS.with(|count| count.set(count.get() + 1));
}

pub(super) fn release_with_fault(lock: ProjectLock) -> Result<(), ProjectLockError> {
    let fingerprint = lock.fingerprint().to_owned();
    lock.release()?;
    if RELEASE_FAULT.with(Cell::get) {
        Err(ProjectLockError::UnlockFailed {
            fingerprint,
            source: io::Error::other(CANARY),
        })
    } else {
        Ok(())
    }
}

pub(crate) fn with_release_fault<T>(run: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            RELEASE_FAULT.with(|flag| flag.set(self.0));
        }
    }
    let _reset = Reset(RELEASE_FAULT.with(|flag| flag.replace(true)));
    run()
}

struct Temp {
    base: PathBuf,
    root: PathBuf,
    locks: PathBuf,
}
impl Temp {
    fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let base = std::env::temp_dir().join(format!(
                "worldbuild-g2-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&base) {
                Ok(()) => {
                    let root = base.join("project");
                    fs::create_dir(&root)?;
                    return Ok(Self {
                        locks: base.join("app-data"),
                        base,
                        root,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "G2 test directory collision",
        ))
    }
    fn runtime(&self) -> Result<ProjectRuntime, RuntimeError> {
        ProjectRuntime::acquire(&self.root, &self.locks)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.base) {
            eprintln!("G2 test cleanup failed: {:?}", error.kind());
        }
    }
}

fn assert_denied(runtime: &mut ProjectRuntime) {
    assert!(!runtime.snapshot().next_action().is_empty());
    let reads = Cell::new(0);
    let writes = Cell::new(0);
    for count in [&reads, &writes] {
        let result = runtime.ready().map(|ready| {
            let _project = ready.locked_project();
            count.set(count.get() + 1);
        });
        assert!(result.is_err());
        assert_eq!(count.get(), 0);
    }
}

fn bytes(title: &str) -> Vec<u8> {
    super::super::json::to_deterministic_json_bytes(&json!({"schemaVersion": 1, "title": title}))
        .expect("valid fixture JSON")
}

#[derive(Clone, Copy)]
enum Fixture {
    Prepared,
    Partial,
    Committed,
    RolledBack,
}

/// M1 prepare가 만든 journal을 검증한 뒤 기존 M1의 partial-apply/cleanup fixture 경로를 쓴다.
fn fixture(temp: &Temp, kind: Fixture) -> Result<PathBuf, Box<dyn Error>> {
    let lock = ProjectLock::try_acquire(&temp.root, &temp.locks)?;
    let project = LockedProject::bind(&lock, &temp.root)?;
    fs::create_dir_all(temp.root.join("data"))?;
    fs::write(temp.root.join("data/a.json"), bytes("old"))?;
    let mut plan = TransactionPlan::new();
    plan.add_json("data/a.json", &json!({"schemaVersion":1,"title":"new"}))?;
    plan.add_json("data/b.json", &json!({"schemaVersion":1,"title":"created"}))?;
    let permit = transaction::test_write_permit(
        project.fingerprint(),
        vec![
            ProjectRelativePath::parse("data/a.json")?,
            ProjectRelativePath::parse("data/b.json")?,
        ],
    );
    let prepared = plan.prepare(&project, permit)?;
    let directory = prepared.transaction_directory().to_path_buf();
    let manifest: TransactionManifest =
        serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    manifest.validate()?;
    assert_eq!(&manifest, prepared.manifest());
    let state: TransactionStateRecord =
        serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
    state.validate()?;
    assert_eq!(state.state, TransactionState::Prepared);
    for operation in &manifest.operations {
        let staged = fs::read(directory.join(&operation.staged_path))?;
        assert_eq!(
            format!("{:x}", Sha256::digest(&staged)),
            operation.staged_sha256
        );
        if let Some(backup) = &operation.backup_path {
            assert_eq!(fs::read(directory.join(backup))?, bytes("old"));
        }
    }
    match kind {
        Fixture::Prepared => drop(prepared),
        Fixture::Partial | Fixture::RolledBack => {
            fs::copy(
                directory.join(&manifest.operations[0].staged_path),
                temp.root.join("data/a.json"),
            )?;
            drop(prepared);
            if matches!(kind, Fixture::RolledBack) {
                let error = with_commit_failures(None, Some(RecoveryTestPoint::Cleanup), || {
                    transaction::test_support::recover_for_runtime_test(&project)
                })
                .expect_err("cleanup fault");
                assert_eq!(
                    error.result_state(),
                    RecoveryResultState::RolledBackCleanupFailed
                );
                assert!(directory.join("rolled-back.json").is_file());
            }
        }
        Fixture::Committed => {
            let outcome = with_commit_failures(Some(CommitTestPoint::Cleanup), None, || {
                transaction::test_support::commit_with_configured_test_hooks(prepared)
            })?;
            assert_eq!(
                outcome.result_state(),
                CommitResultState::CommittedCleanupFailed
            );
            assert!(directory.join("committed.json").is_file());
            assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("new"));
            assert_eq!(fs::read(temp.root.join("data/b.json"))?, bytes("created"));
        }
    }
    lock.release()?;
    Ok(directory)
}

fn assert_block(
    runtime: &mut ProjectRuntime,
    error: &RuntimeError,
    category: RuntimeCategory,
    stage: RecoveryStage,
) {
    assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
    assert_eq!(error.diagnostic().category, category);
    assert_eq!(error.diagnostic().stage, RuntimeStage::Recover(stage));
    assert_eq!(runtime.snapshot().last_failure, Some(error.diagnostic()));
    assert_denied(runtime);
}

#[test]
fn pending_and_recovering_deny_read_and_write_bodies() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
    assert_eq!(
        runtime.ready().unwrap_err().diagnostic().category,
        RuntimeCategory::RecoveryPending
    );
    assert_denied(&mut runtime);
    let before = PHASE_OBSERVATIONS.with(Cell::get);
    runtime.recover()?;
    assert_eq!(PHASE_OBSERVATIONS.with(Cell::get), before + 1);
    assert!(runtime.ready().is_ok());
    runtime.close()?;
    Ok(())
}

#[test]
fn empty_project_initialization_uses_existing_format_without_creating_files() -> TestResult {
    let temp = Temp::new()?;
    let runtime = ProjectRuntime::acquire_empty(&temp.root, &temp.locks, false)?;
    assert_eq!(fs::read_dir(&temp.root)?.count(), 0);
    runtime.close()?;
    Ok(())
}

#[test]
fn occupied_project_initialization_is_rejected_without_touching_entries() -> TestResult {
    let temp = Temp::new()?;
    let existing = temp.root.join("keep.txt");
    fs::write(&existing, CANARY)?;
    let error = ProjectRuntime::acquire_empty(&temp.root, &temp.locks, false)
        .expect_err("occupied folder must not be initialized");
    assert_eq!(
        error.diagnostic().category,
        RuntimeCategory::ProjectNotEmpty
    );
    assert_eq!(error.diagnostic().stage, RuntimeStage::Initialize);
    assert_eq!(fs::read(existing)?, CANARY.as_bytes());
    Ok(())
}

#[test]
fn empty_project_initialization_creates_the_named_directory() -> TestResult {
    let temp = Temp::new()?;
    let target = temp.root.join("새 세계");
    let runtime = ProjectRuntime::acquire_empty(&target, &temp.locks, true)?;
    assert!(target.is_dir());
    assert_eq!(fs::read_dir(&target)?.count(), 0);
    runtime.close()?;
    Ok(())
}

fn invalid_lock_root(temp: &Temp) -> io::Result<PathBuf> {
    let lock_root = temp.base.join("invalid-lock-root");
    fs::write(&lock_root, CANARY)?;
    Ok(lock_root)
}

#[test]
fn created_directory_primary_failure_records_successful_cleanup() -> TestResult {
    let temp = Temp::new()?;
    let target = temp.root.join("cleanup-success");
    let error = ProjectRuntime::acquire_empty(&target, &invalid_lock_root(&temp)?, true)
        .expect_err("invalid lock root must fail");
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.initialization_cleanup_outcome,
        Some(InitializationCleanupOutcome::Removed)
    );
    assert!(diagnostic.initialization_cleanup.is_none());
    assert!(!target.exists());
    assert_redacted(&error);
    Ok(())
}

#[test]
fn created_directory_primary_failure_keeps_cleanup_io_diagnostic() -> TestResult {
    let temp = Temp::new()?;
    let target = temp.root.join("cleanup-io-failure");
    let error = with_create_cleanup_fault(CreateCleanupFault::Io, || {
        ProjectRuntime::acquire_empty(&target, &invalid_lock_root(&temp).unwrap(), true)
            .expect_err("invalid lock root must fail")
    });
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.initialization_cleanup_outcome,
        Some(InitializationCleanupOutcome::Failed)
    );
    assert_eq!(
        diagnostic.initialization_cleanup.unwrap().kind,
        io::ErrorKind::PermissionDenied
    );
    assert!(target.is_dir());
    assert_redacted(&error);
    Ok(())
}

#[test]
fn created_directory_cleanup_preserves_external_entry() -> TestResult {
    let temp = Temp::new()?;
    let target = temp.root.join("cleanup-external-entry");
    let error = with_create_cleanup_fault(CreateCleanupFault::ExternalEntry, || {
        ProjectRuntime::acquire_empty(&target, &invalid_lock_root(&temp).unwrap(), true)
            .expect_err("invalid lock root must fail")
    });
    let diagnostic = error.diagnostic();
    assert_eq!(
        diagnostic.initialization_cleanup_outcome,
        Some(InitializationCleanupOutcome::PreservedExternalEntries)
    );
    assert_eq!(
        diagnostic.initialization_cleanup.unwrap().kind,
        io::ErrorKind::DirectoryNotEmpty
    );
    assert_eq!(fs::read(target.join("external.txt"))?, CANARY.as_bytes());
    assert_redacted(&error);
    Ok(())
}

#[test]
fn no_journal_real_recovery_does_not_create_namespace_or_change_artifacts() -> TestResult {
    let temp = Temp::new()?;
    fs::write(temp.root.join("untouched"), CANARY)?;
    let mut runtime = temp.runtime()?;
    let summary = runtime.recover()?;
    assert_eq!(summary.discovered_transactions, 0);
    let ready = runtime.ready()?;
    assert_eq!(
        ready.locked_project().canonical_root(),
        fs::canonicalize(&temp.root)?
    );
    assert_eq!(fs::read(temp.root.join("untouched"))?, CANARY.as_bytes());
    assert!(!temp.root.join(".worldbuild").exists());
    drop(ready);
    runtime.close()?;
    Ok(())
}

#[test]
fn prepared_and_partial_transactions_restore_old_before_access() -> TestResult {
    for kind in [Fixture::Prepared, Fixture::Partial] {
        let temp = Temp::new()?;
        let directory = fixture(&temp, kind)?;
        let mut runtime = temp.runtime()?;
        assert_denied(&mut runtime);
        assert_eq!(runtime.recover()?.rolled_back_transactions, 1);
        assert!(!directory.exists());
        let ready = runtime.ready()?;
        assert_eq!(
            fs::read(ready.locked_project().canonical_root().join("data/a.json"))?,
            bytes("old")
        );
        assert!(!temp.root.join("data/b.json").exists());
        drop(ready);
        runtime.close()?;
    }
    Ok(())
}

#[test]
fn committed_cleanup_keeps_new_without_reapplying_and_reopens() -> TestResult {
    let temp = Temp::new()?;
    let directory = fixture(&temp, Fixture::Committed)?;
    let mut runtime = temp.runtime()?;
    assert_eq!(runtime.recover()?.committed_cleanups, 1);
    assert!(!directory.exists());
    assert!(runtime.ready().is_ok());
    assert_eq!(runtime.recover()?.discovered_transactions, 0);
    runtime.close()?;
    let mut reopened = temp.runtime()?;
    assert_denied(&mut reopened);
    assert_eq!(reopened.recover()?.discovered_transactions, 0);
    assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("new"));
    assert_eq!(fs::read(temp.root.join("data/b.json"))?, bytes("created"));
    reopened.close()?;
    Ok(())
}

#[test]
fn preparing_orphan_and_rolled_back_marker_are_cleaned() -> TestResult {
    let temp = Temp::new()?;
    let directory = fixture(&temp, Fixture::RolledBack)?;
    let mut runtime = temp.runtime()?;
    // 기존 M1 orphan fixture: 유효한 ID의 디렉터리와 빈 staged, manifest/marker 없음.
    let (id, _) = TransactionId::new_candidate(runtime.lock.fingerprint())?;
    let orphan = temp.root.join(".worldbuild/transactions").join(id.as_str());
    fs::create_dir_all(orphan.join("staged"))?;
    assert!(!orphan.join("manifest.json").exists());
    let summary = runtime.recover()?;
    assert_eq!(summary.rolled_back_cleanups, 1);
    assert_eq!(summary.preparing_orphan_cleanups, 1);
    assert!(!directory.exists() && !orphan.exists());
    assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("old"));
    runtime.close()?;
    Ok(())
}

#[test]
fn enumerate_io_failure_blocks_and_explicit_retry_uses_same_lock() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    let error = with_commit_failures(None, Some(RecoveryTestPoint::Enumerate), || {
        runtime.recover()
    })
    .unwrap_err();
    assert_block(
        &mut runtime,
        &error,
        RuntimeCategory::RecoveryRequired,
        RecoveryStage::EnumerateTransactions,
    );
    assert_eq!(
        temp.runtime().unwrap_err().diagnostic().category,
        RuntimeCategory::AlreadyLocked
    );
    runtime.recover()?;
    assert!(runtime.ready().is_ok());
    assert_eq!(runtime.snapshot().last_failure, None);
    runtime.close()?;
    Ok(())
}

#[test]
fn rollback_io_failure_preserves_journal_until_actual_retry() -> TestResult {
    let temp = Temp::new()?;
    let directory = fixture(&temp, Fixture::Partial)?;
    let mut runtime = temp.runtime()?;
    let error = with_commit_failures(None, Some(RecoveryTestPoint::RestoreExisting), || {
        runtime.recover()
    })
    .unwrap_err();
    assert_block(
        &mut runtime,
        &error,
        RuntimeCategory::RecoveryRequired,
        RecoveryStage::RestoreExistingTarget,
    );
    assert!(directory.join("manifest.json").is_file());
    assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("new"));
    assert_eq!(runtime.recover()?.rolled_back_transactions, 1);
    assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("old"));
    runtime.close()?;
    Ok(())
}

#[test]
fn rolled_back_cleanup_failure_blocks_until_cleanup_retry() -> TestResult {
    let temp = Temp::new()?;
    let directory = fixture(&temp, Fixture::Partial)?;
    let mut runtime = temp.runtime()?;
    let error = with_commit_failures(None, Some(RecoveryTestPoint::Cleanup), || runtime.recover())
        .unwrap_err();
    assert_block(
        &mut runtime,
        &error,
        RuntimeCategory::RolledBackCleanupFailed,
        RecoveryStage::CleanupTransaction,
    );
    assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("old"));
    assert!(directory.join("rolled-back.json").is_file());
    assert_eq!(runtime.recover()?.rolled_back_cleanups, 1);
    assert!(!directory.exists());
    runtime.close()?;
    Ok(())
}

#[test]
fn committed_cleanup_error_preserves_m1_variant_without_claiming_old_data() -> TestResult {
    let temp = Temp::new()?;
    let directory = fixture(&temp, Fixture::Committed)?;
    let mut runtime = temp.runtime()?;
    let error = with_commit_failures(None, Some(RecoveryTestPoint::Cleanup), || runtime.recover())
        .unwrap_err();
    assert_block(
        &mut runtime,
        &error,
        RuntimeCategory::RolledBackCleanupFailed,
        RecoveryStage::CleanupTransaction,
    );
    assert_eq!(
        error.diagnostic().recovery_result,
        Some(RecoveryResultState::RolledBackCleanupFailed)
    );
    assert!(directory.join("committed.json").is_file());
    assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("new"));
    assert_eq!(runtime.recover()?.committed_cleanups, 1);
    runtime.close()?;
    Ok(())
}

#[test]
fn corrupt_journal_is_preserved_and_closing_does_not_report_recovery() -> TestResult {
    let temp = Temp::new()?;
    let directory = fixture(&temp, Fixture::Prepared)?;
    let path = directory.join("manifest.json");
    let valid = fs::read(&path)?;
    fs::write(&path, CANARY)?;
    let mut runtime = temp.runtime()?;
    let error = runtime.recover().unwrap_err();
    assert_block(
        &mut runtime,
        &error,
        RuntimeCategory::ManualRecoveryRequired,
        RecoveryStage::InspectJournal,
    );
    assert_eq!(fs::read(&path)?, CANARY.as_bytes());
    let closed = runtime.close()?;
    assert_eq!(closed.state, RuntimeState::Blocked);
    let mut reopened = temp.runtime()?;
    assert_denied(&mut reopened);
    assert!(reopened.recover().is_err());
    // 원인 해소는 테스트에서만 수행한다. runtime은 repair/delete API를 제공하지 않는다.
    fs::write(&path, valid)?;
    reopened.recover()?;
    assert!(!directory.exists());
    reopened.close()?;
    Ok(())
}

#[test]
fn invalid_namespace_and_entry_are_never_an_empty_report() -> TestResult {
    for relative in [
        ".worldbuild",
        ".worldbuild/transactions",
        ".worldbuild/transactions/invalid-name",
    ] {
        let temp = Temp::new()?;
        let path = temp.root.join(relative);
        fs::create_dir_all(path.parent().ok_or("parent")?)?;
        fs::write(&path, CANARY)?;
        let mut runtime = temp.runtime()?;
        let error = runtime.recover().unwrap_err();
        assert_eq!(
            error.diagnostic().category,
            RuntimeCategory::ManualRecoveryRequired
        );
        assert_eq!(runtime.snapshot().last_report, None);
        assert_denied(&mut runtime);
        assert_eq!(fs::read(&path)?, CANARY.as_bytes());
        runtime.close()?;
    }
    Ok(())
}

#[test]
fn successful_outer_report_requiring_manual_recovery_stays_blocked() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    // 현재 M1이 생성하지 않는 방어 분기만 private result handler로 검사한다.
    runtime.state = RuntimeState::Recovering;
    let error = runtime
        .finish_recovery(RecoveryReport {
            manual_recovery_required: true,
            ..RecoveryReport::default()
        })
        .unwrap_err();
    assert_eq!(error.diagnostic().stage, RuntimeStage::ReviewReport);
    assert_eq!(
        error.diagnostic().category,
        RuntimeCategory::ManualRecoveryRequired
    );
    assert_denied(&mut runtime);
    runtime.recover()?;
    runtime.close()?;
    Ok(())
}

#[test]
fn invalidation_and_ready_recheck_cannot_reuse_old_authority() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    runtime.recover()?;
    let observation = runtime.snapshot();
    runtime.invalidate_recovery();
    assert_eq!(observation.state, RuntimeState::Ready);
    assert_denied(&mut runtime);
    runtime.recover()?;
    let error = with_commit_failures(None, Some(RecoveryTestPoint::Enumerate), || {
        runtime.recover()
    })
    .unwrap_err();
    assert_block(
        &mut runtime,
        &error,
        RuntimeCategory::RecoveryRequired,
        RecoveryStage::EnumerateTransactions,
    );
    runtime.close()?;
    Ok(())
}

#[test]
fn duplicate_open_blocks_until_explicit_close_and_keeps_lock_files() -> TestResult {
    let temp = Temp::new()?;
    let runtime = temp.runtime()?;
    let lock_path = runtime.lock.lock_path().to_path_buf();
    let metadata_path = runtime.lock.metadata_path().to_path_buf();
    assert_eq!(
        temp.runtime().unwrap_err().diagnostic().category,
        RuntimeCategory::AlreadyLocked
    );
    assert_eq!(runtime.close()?.state, RuntimeState::Pending);
    assert!(lock_path.is_file() && metadata_path.is_file());
    temp.runtime()?.close()?;
    Ok(())
}

#[test]
fn lock_failure_never_begins_recovery() -> TestResult {
    let temp = Temp::new()?;
    fs::write(&temp.locks, CANARY)?;
    let before = PHASE_OBSERVATIONS.with(Cell::get);
    assert_eq!(
        temp.runtime().unwrap_err().diagnostic().category,
        RuntimeCategory::LockDirectoryUnavailable
    );
    assert_eq!(PHASE_OBSERVATIONS.with(Cell::get), before);
    assert!(!temp.root.join(".worldbuild").exists());
    Ok(())
}

#[test]
fn recovery_binding_failure_keeps_runtime_blocked_and_retryable() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    fs::remove_dir(&temp.root)?;
    let error = runtime.recover().unwrap_err();
    assert_eq!(
        error.diagnostic().category,
        RuntimeCategory::ProjectBindingFailed
    );
    assert_eq!(error.diagnostic().stage, RuntimeStage::Bind);
    assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
    assert_denied(&mut runtime);
    fs::create_dir(&temp.root)?;
    assert_eq!(
        temp.runtime().unwrap_err().diagnostic().category,
        RuntimeCategory::AlreadyLocked
    );
    runtime.recover()?;
    runtime.close()?;
    Ok(())
}

#[test]
fn ready_binding_failure_blocks_restored_partial_journal_until_explicit_recovery() -> TestResult {
    let temp = Temp::new()?;
    // fixture()가 실제 M1 prepare의 manifest/state/hash/backup을 검증한 뒤 일부만 적용한다.
    let directory = fixture(&temp, Fixture::Partial)?;
    let canonical_root = fs::canonicalize(&temp.root)?;
    let relative_journal = directory.strip_prefix(&canonical_root)?.to_path_buf();
    let manifest_bytes = fs::read(directory.join("manifest.json"))?;
    let manifest: TransactionManifest = serde_json::from_slice(&manifest_bytes)?;
    let state_bytes = fs::read(directory.join("state.json"))?;
    let assert_partial_unchanged = || -> TestResult {
        // 디스크 관찰은 test harness 책임이며 아래 protected read/write body와 구별한다.
        assert_eq!(fs::read(directory.join("manifest.json"))?, manifest_bytes);
        assert_eq!(fs::read(directory.join("state.json"))?, state_bytes);
        assert!(!directory.join("committed.json").exists());
        assert!(!directory.join("rolled-back.json").exists());
        assert_eq!(fs::read(temp.root.join("data/a.json"))?, bytes("new"));
        assert!(!temp.root.join("data/b.json").exists());
        for operation in &manifest.operations {
            assert_eq!(
                format!(
                    "{:x}",
                    Sha256::digest(fs::read(directory.join(&operation.staged_path))?)
                ),
                operation.staged_sha256
            );
            if let Some(backup) = &operation.backup_path {
                assert_eq!(fs::read(directory.join(backup))?, bytes("old"));
            }
        }
        Ok(())
    };
    assert_partial_unchanged()?;
    let holding = temp.base.join("holding");
    let base = fs::canonicalize(&temp.base)?;
    assert!(canonical_root.starts_with(&base));
    assert_eq!(holding.parent(), Some(temp.base.as_path()));
    // fixture의 lock/handle은 이미 해제됐다. 이동 대상은 이 테스트의 임시 base뿐이다.
    fs::rename(&temp.root, &holding)?;
    fs::create_dir(&temp.root)?;
    let mut runtime = temp.runtime()?;
    assert_eq!(runtime.recover()?.discovered_transactions, 0);
    let prior = runtime.snapshot();
    let phase_count = PHASE_OBSERVATIONS.with(Cell::get);
    for _ in 0..2 {
        let ready = runtime.ready()?;
        assert_eq!(
            ready.locked_project().fingerprint(),
            manifest.project_fingerprint
        );
    }
    assert_eq!(runtime.snapshot(), prior);
    assert_eq!(PHASE_OBSERVATIONS.with(Cell::get), phase_count);
    fs::remove_dir(&temp.root)?;
    let error = runtime.ready().unwrap_err();
    assert_eq!(
        error.diagnostic().category,
        RuntimeCategory::ProjectBindingFailed
    );
    assert_eq!(error.diagnostic().stage, RuntimeStage::Bind);
    assert_eq!(
        error.diagnostic().io.ok_or("binding I/O")?.kind,
        io::ErrorKind::NotFound
    );
    assert_eq!(
        runtime.snapshot().state,
        RuntimeState::Blocked,
        "binding failure must revoke readiness"
    );
    assert_eq!(runtime.snapshot().last_failure, Some(error.diagnostic()));
    assert_eq!(runtime.snapshot().last_report, prior.last_report);
    assert_ne!(runtime.snapshot().next_action(), prior.next_action());
    assert!(runtime.snapshot().next_action().contains("복구"));
    assert_redacted(&error);
    assert_denied(&mut runtime);
    assert_eq!(
        runtime.ready().unwrap_err().diagnostic(),
        error.diagnostic()
    );
    assert!(fs::canonicalize(&holding)?.starts_with(&base));
    fs::rename(&holding, &temp.root)?;
    assert_eq!(fs::canonicalize(&temp.root)?, canonical_root);
    assert_eq!(temp.root.join(&relative_journal).canonicalize()?, directory);
    assert_eq!(
        temp.runtime().unwrap_err().diagnostic().category,
        RuntimeCategory::AlreadyLocked
    );
    assert_partial_unchanged()?;
    assert_denied(&mut runtime);
    assert_eq!(
        runtime.ready().unwrap_err().diagnostic(),
        error.diagnostic()
    );
    assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
    assert_eq!(runtime.snapshot().last_failure, Some(error.diagnostic()));
    assert_partial_unchanged()?;
    assert_eq!(PHASE_OBSERVATIONS.with(Cell::get), phase_count);
    assert_eq!(runtime.recover()?.rolled_back_transactions, 1);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    assert_eq!(runtime.snapshot().last_failure, None);
    assert!(!directory.exists());
    let mut reads = 0;
    let mut writes = 0;
    runtime.ready().map(|ready| {
        let actual = fs::read(ready.locked_project().canonical_root().join("data/a.json"))?;
        assert_eq!(actual, bytes("old"));
        reads += 1;
        Ok::<(), io::Error>(())
    })??;
    runtime.ready().map(|ready| {
        std::hint::black_box(ready.locked_project());
        writes += 1;
    })?;
    assert_eq!((reads, writes), (1, 1));
    assert_eq!(runtime.recover()?.discovered_transactions, 0);
    runtime.close()?;
    Ok(())
}

#[test]
fn ready_binding_failure_close_keeps_cause_and_separates_release_failure() -> TestResult {
    for root_is_file in [false, true] {
        for release_fails in [false, true] {
            let temp = Temp::new()?;
            let mut runtime = temp.runtime()?;
            runtime.recover()?;
            fs::remove_dir(&temp.root)?;
            if root_is_file {
                fs::write(&temp.root, CANARY)?;
            }
            let binding = runtime.ready().unwrap_err();
            assert_eq!(
                binding.diagnostic().category,
                RuntimeCategory::ProjectBindingFailed
            );
            assert_eq!(binding.diagnostic().stage, RuntimeStage::Bind);
            assert_eq!(
                binding.diagnostic().io.ok_or("binding I/O")?.kind,
                if root_is_file {
                    io::ErrorKind::NotADirectory
                } else {
                    io::ErrorKind::NotFound
                }
            );
            assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
            assert_denied(&mut runtime);
            assert_eq!(
                runtime.ready().unwrap_err().diagnostic(),
                binding.diagnostic()
            );
            assert_redacted(&binding);
            assert!(!format!("{runtime:?}").contains(&temp.base.to_string_lossy().to_string()));
            let result = if release_fails {
                with_release_fault(|| runtime.close())
            } else {
                runtime.close()
            };
            let snapshot = if release_fails {
                let close = result.unwrap_err();
                assert_eq!(
                    close
                        .previous_failure
                        .as_ref()
                        .ok_or("binding cause")?
                        .diagnostic(),
                    binding.diagnostic()
                );
                assert_eq!(
                    close.release.diagnostic().category,
                    RuntimeCategory::ReleaseFailed
                );
                assert_redacted(&close);
                close.snapshot
            } else {
                result?
            };
            assert_eq!(snapshot.state, RuntimeState::Blocked);
            assert_eq!(snapshot.last_failure, Some(binding.diagnostic()));
            if root_is_file {
                assert_eq!(fs::read(&temp.root)?, CANARY.as_bytes());
                fs::remove_file(&temp.root)?;
            }
            fs::create_dir(&temp.root)?;
            temp.runtime()?.close()?;
        }
    }
    Ok(())
}

#[test]
fn binding_mismatch_explicitly_releases_acquired_lock() -> TestResult {
    let temp = Temp::new()?;
    let other = temp.base.join("other-project");
    fs::create_dir(&other)?;
    let lock = ProjectLock::try_acquire(&temp.root, &temp.locks)?;
    let error = ProjectRuntime::bind_acquired(lock, &other).unwrap_err();
    assert_eq!(
        error.diagnostic().category,
        RuntimeCategory::ProjectIdentityMismatch
    );
    temp.runtime()?.close()?;
    Ok(())
}

#[test]
fn binding_and_release_failure_causes_are_kept_separately() -> TestResult {
    let temp = Temp::new()?;
    let lock = ProjectLock::try_acquire(&temp.root, &temp.locks)?;
    let error =
        with_release_fault(|| ProjectRuntime::bind_acquired(lock, &temp.base.join("missing")))
            .unwrap_err();
    assert_eq!(
        error.diagnostic().category,
        RuntimeCategory::ProjectBindingFailed
    );
    assert_eq!(
        error.diagnostic().io.ok_or("binding I/O")?.kind,
        io::ErrorKind::NotFound
    );
    assert_eq!(
        error
            .diagnostic()
            .release_failure
            .ok_or("release I/O")?
            .kind,
        io::ErrorKind::Other
    );
    assert_redacted(&error);
    temp.runtime()?.close()?;
    Ok(())
}

#[test]
fn blocked_close_release_failure_preserves_previous_failure_and_consumes_guard() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    let prior = with_commit_failures(None, Some(RecoveryTestPoint::Enumerate), || {
        runtime.recover()
    })
    .unwrap_err();
    let error = with_release_fault(|| runtime.close()).unwrap_err();
    assert_eq!(error.snapshot.state, RuntimeState::Blocked);
    assert_eq!(
        error
            .previous_failure
            .as_ref()
            .ok_or("prior failure")?
            .diagnostic(),
        prior.diagnostic()
    );
    assert_eq!(
        error.release.diagnostic().category,
        RuntimeCategory::ReleaseFailed
    );
    assert_redacted(&error);
    temp.runtime()?.close()?;
    Ok(())
}

pub(super) fn assert_redacted(error: &dyn Error) {
    let mut next = Some(error);
    while let Some(error) = next {
        let output = format!("{error:?}\n{error}");
        for secret in [
            CANARY,
            "private-user",
            "raw-journal-body",
            "credential=secret",
        ] {
            assert!(!output.contains(secret), "diagnostic leaked a canary");
        }
        next = error.source();
    }
}

#[test]
fn recovery_binding_and_lock_diagnostics_hide_raw_sources_and_paths() -> TestResult {
    let temp = Temp::new()?;
    let mut runtime = temp.runtime()?;
    let path = temp.root.join(".worldbuild");
    fs::write(&path, CANARY)?;
    let error = runtime.recover().unwrap_err();
    assert_redacted(&error);
    let output = format!("{runtime:?} {:?}", runtime.snapshot());
    assert!(!output.contains(&temp.base.to_string_lossy().to_string()));
    runtime.close()?;
    let error = RuntimeError::binding(transaction::TransactionModelError::ProjectBindingFailed {
        lock_fingerprint: CANARY.into(),
        source: io::Error::other(CANARY),
    });
    assert_redacted(&error);
    let error = RuntimeError::lock(ProjectLockError::InvalidProjectPath {
        path: CANARY.into(),
        source: io::Error::other(CANARY),
    });
    assert_redacted(&error);
    Ok(())
}
