use std::any::Any;
use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::Serialize;

use super::super::atomic_file::{SaveError, SaveOutcome};
use super::super::json::to_deterministic_json_bytes;
use super::super::project_lock::ProjectLock;
use super::apply::{commit_with_hooks, CommitFailPoint, CommitFailureSource, CommitHooks};
use super::recovery::RecoveryFailPoint;
use super::*;
use crate::data::collaboration_lock::{
    HeldLock, HeldLockState, LockAcquireRequest, LockCapabilities, LockCoordinator, LockError,
    LockErrorCategory, LockOperation, LockProviderInfo, LockProviderKind, LockService,
    LockSessionId, LockSetGuard,
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
type TestResult = Result<(), Box<dyn std::error::Error>>;

struct ScriptedLockService {
    instance_id: u64,
    validation_calls: AtomicUsize,
    fail_validation_at: AtomicUsize,
    failure_category: AtomicUsize,
}

impl ScriptedLockService {
    fn new() -> Self {
        Self {
            instance_id: TEST_COUNTER.fetch_add(1, Ordering::Relaxed) + 10_000,
            validation_calls: AtomicUsize::new(0),
            fail_validation_at: AtomicUsize::new(0),
            failure_category: AtomicUsize::new(1),
        }
    }

    fn fail_at(&self, call: usize) {
        self.fail_validation_at.store(call, Ordering::SeqCst);
    }

    fn fail_at_with_category(&self, call: usize, category: LockErrorCategory) {
        let encoded = match category {
            LockErrorCategory::LockLost => 1,
            LockErrorCategory::LockStateUnknown => 2,
            LockErrorCategory::LockValidationFailed => 3,
            _ => panic!("unsupported scripted validation category"),
        };
        self.failure_category.store(encoded, Ordering::SeqCst);
        self.fail_at(call);
    }
}

struct ScriptedHeldLock {
    instance_id: u64,
    project_fingerprint: String,
    session_id: LockSessionId,
    target: ProjectRelativePath,
}

impl HeldLock for ScriptedHeldLock {
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

impl LockService for ScriptedLockService {
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
        Ok(Box::new(ScriptedHeldLock {
            instance_id: self.instance_id,
            project_fingerprint: request.project_fingerprint().to_owned(),
            session_id: request.session_id().clone(),
            target: request.target().clone(),
        }))
    }

    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let call = self.validation_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if call == self.fail_validation_at.load(Ordering::SeqCst) {
            let category = match self.failure_category.load(Ordering::SeqCst) {
                1 => LockErrorCategory::LockLost,
                2 => LockErrorCategory::LockStateUnknown,
                _ => LockErrorCategory::LockValidationFailed,
            };
            return Err(LockError::for_held(
                category,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Document<'a> {
    schema_version: u32,
    title: &'a str,
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "worldbuild-transaction-commit-{}-{count}",
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
            "test collision",
        ))
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

fn with_project<F>(test: F) -> TestResult
where
    F: FnOnce(&Path, &LockedProject<'_>) -> TestResult,
{
    let temp = TestDirectory::new()?;
    let root = temp.0.join("private-project-name");
    fs::create_dir(&root)?;
    fs::create_dir(root.join("data"))?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    test(&root, &project)
}

fn bytes(title: &str) -> Result<Vec<u8>, serde_json::Error> {
    to_deterministic_json_bytes(&Document {
        schema_version: 1,
        title,
    })
}

fn plan(entries: &[(&str, &str)]) -> Result<TransactionPlan, TransactionPrepareError> {
    let mut plan = TransactionPlan::new();
    for (path, title) in entries {
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

fn acquire_scripted_guard(
    project: &LockedProject<'_>,
    service: Arc<ScriptedLockService>,
    entries: &[(&str, &str)],
) -> Result<LockSetGuard, Box<dyn std::error::Error>> {
    let provider: Arc<dyn LockService> = service;
    let coordinator = LockCoordinator::new(provider);
    let session = LockSessionId::generate()?;
    let targets = entries
        .iter()
        .map(|(target, _)| ProjectRelativePath::parse(target))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(coordinator.acquire_all(project.fingerprint(), &session, targets)?)
}

struct ObserveMarker<'a> {
    root: &'a Path,
    observed_all_staged: RefCell<bool>,
}

impl CommitHooks for ObserveMarker<'_> {
    fn check(&self, point: CommitFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == CommitFailPoint::CommittedMarkerWrite {
            let all_staged = ["a", "b", "c"].iter().all(|name| {
                fs::read(self.root.join(format!("data/{name}.json")))
                    .ok()
                    .as_deref()
                    == bytes(&format!("new-{name}")).ok().as_deref()
            });
            self.observed_all_staged.replace(all_staged);
        }
        Ok(())
    }
}

#[test]
fn prepare_lock_validation_failure_creates_no_filesystem_artifacts() -> TestResult {
    with_project(|root, project| {
        // 부모가 없는 경로이므로 permit보다 먼저 resolve/canonicalize하면 다른 오류가 발생한다.
        let entries = [("missing/item.json", "new")];
        let service = Arc::new(ScriptedLockService::new());
        let mut guard = acquire_scripted_guard(project, service.clone(), &entries)?;
        let permit = guard.write_permit()?;
        service.fail_at(2);
        let error = plan(&entries)?
            .prepare(project, permit)
            .err()
            .ok_or("invalid permit unexpectedly prepared")?;
        assert!(matches!(error, TransactionPrepareError::Permit(_)));
        assert_eq!(service.validation_calls.load(Ordering::SeqCst), 2);
        assert!(!root.join("missing/item.json").exists());
        assert!(!root.join(".worldbuild").exists());
        Ok(())
    })
}

#[test]
fn commit_entry_fails_closed_for_unknown_and_provider_validation_failure() -> TestResult {
    for category in [
        LockErrorCategory::LockStateUnknown,
        LockErrorCategory::LockValidationFailed,
    ] {
        with_project(|root, project| {
            let entries = [("data/item.json", "new")];
            fs::write(root.join("data/item.json"), bytes("old")?)?;
            let service = Arc::new(ScriptedLockService::new());
            let mut guard = acquire_scripted_guard(project, service.clone(), &entries)?;
            let permit = guard.write_permit()?;
            let prepared = plan(&entries)?.prepare(project, permit)?;
            service.fail_at_with_category(3, category);
            let error = prepared.commit().expect_err("validation must fail closed");
            assert_eq!(error.result_state(), CommitResultState::NotApplied);
            assert_eq!(error.failure().stage, CommitStage::ValidateCommitEntryLock);
            assert_eq!(fs::read(root.join("data/item.json"))?, bytes("old")?);
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn lock_loss_at_commit_validation_boundaries_never_leaves_partial_data() -> TestResult {
    for (fail_at, expected_state, expected_stage, expected_applied) in [
        (
            7,
            CommitResultState::NotApplied,
            CommitStage::ValidateCommitEntryLock,
            0,
        ),
        (
            10,
            CommitResultState::NotApplied,
            CommitStage::ValidateOperationLock,
            0,
        ),
        (
            13,
            CommitResultState::RolledBack,
            CommitStage::ValidateOperationLock,
            1,
        ),
        (
            16,
            CommitResultState::RolledBack,
            CommitStage::ValidateOperationLock,
            2,
        ),
        (
            19,
            CommitResultState::RolledBack,
            CommitStage::ValidateBeforeMarkerLock,
            3,
        ),
    ] {
        with_project(|root, project| {
            let entries = [
                ("data/a.json", "new-a"),
                ("data/b.json", "new-b"),
                ("data/c.json", "new-c"),
            ];
            for (target, _) in entries {
                fs::write(root.join(target), bytes("old")?)?;
            }
            let service = Arc::new(ScriptedLockService::new());
            let mut guard = acquire_scripted_guard(project, service.clone(), &entries)?;
            let permit = guard.write_permit()?;
            let prepared = plan(&entries)?.prepare(project, permit)?;
            let transaction_directory = prepared.transaction_directory().to_path_buf();
            service.fail_at(fail_at);
            let hooks = ObserveMarker {
                root,
                observed_all_staged: RefCell::new(false),
            };

            match commit_with_hooks(prepared, &hooks) {
                Err(error) => {
                    assert_eq!(error.result_state(), expected_state);
                    assert_eq!(error.failure().stage, expected_stage);
                    assert_eq!(error.failure().applied_operations().len(), expected_applied);
                }
                Ok(outcome) => {
                    assert_eq!(outcome.result_state(), expected_state);
                    let (failure, _) = outcome
                        .rollback_failures()
                        .ok_or("rollback diagnostics missing")?;
                    assert_eq!(failure.stage, expected_stage);
                    assert_eq!(failure.applied_operations().len(), expected_applied);
                }
            }
            assert_eq!(service.validation_calls.load(Ordering::SeqCst), fail_at);
            for name in ["a", "b", "c"] {
                assert_eq!(
                    fs::read(root.join(format!("data/{name}.json")))?,
                    bytes("old")?
                );
            }
            if expected_state == CommitResultState::NotApplied {
                assert!(transaction_directory.join("manifest.json").is_file());
                assert!(transaction_directory.join("backups/000000.json").is_file());
                assert!(!transaction_directory.join("committed.json").exists());
            } else {
                assert!(!transaction_directory.exists());
            }
            assert_eq!(*hooks.observed_all_staged.borrow(), fail_at == 19);
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn lock_error_and_rollback_error_are_both_preserved() -> TestResult {
    with_project(|root, project| {
        let entries = [("data/a.json", "new-a"), ("data/b.json", "new-b")];
        for (target, _) in entries {
            fs::write(root.join(target), bytes("old")?)?;
        }
        let service = Arc::new(ScriptedLockService::new());
        let mut guard = acquire_scripted_guard(project, service.clone(), &entries)?;
        let permit = guard.write_permit()?;
        let prepared = plan(&entries)?.prepare(project, permit)?;
        let transaction_directory = prepared.transaction_directory().to_path_buf();
        service.fail_at(9);
        let error = commit_with_hooks(prepared, &RollbackOnlyFail)
            .expect_err("rollback failure must require recovery");
        assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
        assert_eq!(error.failure().stage, CommitStage::ValidateOperationLock);
        assert!(matches!(
            error.failure().source.as_ref(),
            CommitFailureSource::Permit(WritePermitError::Validation(_))
        ));
        assert!(error.rollback_failure().is_some());
        assert!(transaction_directory.join("manifest.json").is_file());
        assert!(transaction_directory.join("backups/000000.json").is_file());
        assert!(transaction_directory.join("backups/000001.json").is_file());
        assert!(!transaction_directory.join("committed.json").exists());
        Ok(())
    })
}

struct RollbackOnlyFail;

impl CommitHooks for RollbackOnlyFail {
    fn check(&self, _point: CommitFailPoint, _operation: Option<u32>) -> io::Result<()> {
        Ok(())
    }

    fn check_recovery(&self, point: RecoveryFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == RecoveryFailPoint::RollingBackState {
            Err(io::Error::other("injected rollback failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn lock_loss_after_creating_a_new_target_removes_it_during_rollback() -> TestResult {
    with_project(|root, project| {
        let entries = [("data/a.json", "new-a"), ("data/b.json", "new-b")];
        fs::write(root.join("data/a.json"), bytes("old")?)?;
        let service = Arc::new(ScriptedLockService::new());
        let mut guard = acquire_scripted_guard(project, service.clone(), &entries)?;
        let permit = guard.write_permit()?;
        let prepared = plan(&entries)?.prepare(project, permit)?;
        service.fail_at(9);
        let outcome = prepared.commit()?;
        assert_eq!(outcome.result_state(), CommitResultState::RolledBack);
        assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old")?);
        assert!(!root.join("data/b.json").exists());
        Ok(())
    })
}

#[test]
fn commits_existing_new_and_mixed_files_in_deterministic_order() -> TestResult {
    for existing in [0_usize, 3, 1] {
        with_project(|root, project| {
            let entries = [
                ("data/c.json", "new-c"),
                ("data/a.json", "new-a"),
                ("data/b.json", "new-b"),
            ];
            for (path, _) in entries.iter().take(existing) {
                fs::write(root.join(path), bytes("old")?)?;
            }
            let prepared = plan(&entries)?.prepare_for_test(project)?;
            assert_eq!(
                prepared
                    .manifest()
                    .operations
                    .iter()
                    .map(|operation| operation.target_path.as_str())
                    .collect::<Vec<_>>(),
                vec!["data/a.json", "data/b.json", "data/c.json"]
            );
            let transaction_directory = prepared.transaction_directory().to_path_buf();
            assert!(matches!(prepared.commit()?, CommitOutcome::Committed));
            for (path, title) in entries {
                assert_eq!(fs::read(root.join(path))?, bytes(title)?);
            }
            assert!(!transaction_directory.exists());
            assert!(root.join(".worldbuild/transactions").is_dir());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn precommit_corruption_and_existing_markers_are_not_applied() -> TestResult {
    for (artifact, remove) in [
        ("manifest.json", false),
        ("staged/000000.json", false),
        ("staged/000000.json", true),
        ("backups/000000.json", false),
        ("backups/000000.json", true),
        ("state.json", false),
    ] {
        with_project(|root, project| {
            let target = root.join("data/a.json");
            let original = bytes("old")?;
            fs::write(&target, &original)?;
            let prepared = plan(&[("data/a.json", "new")])?.prepare_for_test(project)?;
            let artifact_path = prepared.transaction_directory().join(artifact);
            if remove {
                fs::remove_file(artifact_path)?;
            } else {
                fs::write(artifact_path, b"corrupt")?;
            }
            let error = prepared.commit().expect_err("corruption must fail");
            assert_eq!(error.result_state(), CommitResultState::NotApplied);
            assert_eq!(fs::read(target)?, original);
            Ok(())
        })?;
    }
    for marker in ["committed.json", "rolled-back.json"] {
        with_project(|root, project| {
            let target = root.join("data/a.json");
            let original = bytes("old")?;
            fs::write(&target, &original)?;
            let prepared = plan(&[("data/a.json", "new")])?.prepare_for_test(project)?;
            fs::write(prepared.transaction_directory().join(marker), b"present")?;
            let error = prepared.commit().expect_err("marker must fail");
            assert_eq!(error.result_state(), CommitResultState::NotApplied);
            assert_eq!(fs::read(target)?, original);
            Ok(())
        })?;
    }
    Ok(())
}

struct Hooks {
    failure: CommitFailPoint,
    operation: Option<u32>,
    mutation: RefCell<Option<(PathBuf, Vec<u8>)>>,
}

struct SensitiveMarkerFailure {
    point: CommitFailPoint,
    message: String,
}

impl CommitHooks for SensitiveMarkerFailure {
    fn check(&self, point: CommitFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == self.point {
            Err(io::Error::other(self.message.clone()))
        } else {
            Ok(())
        }
    }
}

struct ReplaceMarkerWithDirectory {
    path: PathBuf,
}

impl CommitHooks for ReplaceMarkerWithDirectory {
    fn check(&self, point: CommitFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == CommitFailPoint::CommittedMarkerVerify {
            fs::remove_file(&self.path)?;
            fs::create_dir(&self.path)?;
        }
        Ok(())
    }
}

impl CommitHooks for Hooks {
    fn check(&self, point: CommitFailPoint, operation: Option<u32>) -> io::Result<()> {
        if point == self.failure && (self.operation.is_none() || self.operation == operation) {
            if let Some((path, content)) = self.mutation.borrow_mut().take() {
                fs::write(path, content)?;
                return Ok(());
            }
            return Err(io::Error::other(format!("injected {point:?}")));
        }
        Ok(())
    }
}

fn failing(point: CommitFailPoint, operation: Option<u32>) -> Hooks {
    Hooks {
        failure: point,
        operation,
        mutation: RefCell::new(None),
    }
}

#[test]
fn target_preconditions_distinguish_first_and_middle_changes() -> TestResult {
    for index in [0_u32, 1] {
        with_project(|root, project| {
            for name in ["a", "b", "c"] {
                fs::write(root.join(format!("data/{name}.json")), bytes("old")?)?;
            }
            let prepared = plan(&[
                ("data/a.json", "new-a"),
                ("data/b.json", "new-b"),
                ("data/c.json", "new-c"),
            ])?
            .prepare_for_test(project)?;
            let hooks = Hooks {
                failure: CommitFailPoint::TargetPrecondition,
                operation: Some(index),
                mutation: RefCell::new(Some((
                    root.join(if index == 0 {
                        "data/a.json"
                    } else {
                        "data/b.json"
                    }),
                    bytes("external")?,
                ))),
            };
            let transaction_directory = prepared.transaction_directory().to_path_buf();
            let error = commit_with_hooks(prepared, &hooks).expect_err("precondition must fail");
            assert_eq!(
                error.result_state(),
                if index == 0 {
                    CommitResultState::NotApplied
                } else {
                    CommitResultState::RecoveryRequired
                }
            );
            assert_eq!(error.failure().applied_operations().len(), index as usize);
            assert!(transaction_directory.is_dir());
            assert!(transaction_directory.join("manifest.json").is_file());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn apply_and_pre_decision_marker_failures_roll_back() -> TestResult {
    let cases = [
        (CommitFailPoint::Rename, Some(0)),
        (CommitFailPoint::Rename, Some(1)),
        (CommitFailPoint::Rename, Some(2)),
        (CommitFailPoint::TargetSync, Some(0)),
        (CommitFailPoint::TargetParentSync, Some(0)),
        (CommitFailPoint::AppliedVerification, Some(0)),
        (CommitFailPoint::ProgressState, Some(0)),
        (CommitFailPoint::CommittedMarkerWrite, None),
    ];
    for (point, operation) in cases {
        with_project(|root, project| {
            for name in ["a", "b", "c"] {
                fs::write(root.join(format!("data/{name}.json")), bytes("old")?)?;
            }
            let prepared = plan(&[
                ("data/a.json", "new-a"),
                ("data/b.json", "new-b"),
                ("data/c.json", "new-c"),
            ])?
            .prepare_for_test(project)?;
            let directory = prepared.transaction_directory().to_path_buf();
            let outcome = commit_with_hooks(prepared, &failing(point, operation))?;
            assert_eq!(
                outcome.result_state(),
                CommitResultState::RolledBack,
                "{point:?}"
            );
            assert!(outcome.rollback_failures().is_some());
            assert!(!directory.exists(), "{point:?}");
            for name in ["a", "b", "c"] {
                assert_eq!(
                    fs::read(root.join(format!("data/{name}.json")))?,
                    bytes("old")?,
                    "{point:?}"
                );
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn marker_post_decision_hook_failures_preserve_committed_state_and_recover_forward() -> TestResult {
    for point in [
        CommitFailPoint::CommittedMarkerVerify,
        CommitFailPoint::CommittedMarkerSync,
    ] {
        with_project(|root, project| {
            let original_a = bytes("old-a")?;
            let original_b = bytes("old-b")?;
            let staged_a = bytes("new-a")?;
            let staged_b = bytes("new-b")?;
            fs::write(root.join("data/a.json"), &original_a)?;
            fs::write(root.join("data/b.json"), &original_b)?;
            let prepared = plan(&[("data/a.json", "new-a"), ("data/b.json", "new-b")])?
                .prepare_for_test(project)?;
            let transaction_id = prepared.transaction_id().clone();
            let directory = prepared.transaction_directory().to_path_buf();
            let sensitive_message = format!("post-decision failure at {}", root.display());
            let error = commit_with_hooks(
                prepared,
                &SensitiveMarkerFailure {
                    point,
                    message: sensitive_message.clone(),
                },
            )
            .expect_err("post-decision marker failure must require recovery");

            assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
            assert!(error.rollback_failure().is_none());
            assert_eq!(error.failure().transaction_id, transaction_id);
            assert_eq!(error.failure().applied_operations(), &[0, 1]);
            assert_eq!(
                error.failure().stage,
                if point == CommitFailPoint::CommittedMarkerVerify {
                    CommitStage::VerifyCommittedMarker
                } else {
                    CommitStage::SyncCommittedMarker
                }
            );
            match error.failure().source.as_ref() {
                CommitFailureSource::Io(source) => {
                    assert_eq!(source.to_string(), sensitive_message);
                }
                other => return Err(format!("unexpected marker failure source: {other:?}").into()),
            }
            let diagnostic = format!("{error}\n{error:?}");
            assert!(!diagnostic.contains(&root.display().to_string()));
            let source = std::error::Error::source(&error)
                .ok_or("safe marker failure source category is missing")?;
            assert!(!source.to_string().contains(&root.display().to_string()));
            assert!(source.source().is_none());

            let marker: CommittedMarker =
                serde_json::from_slice(&fs::read(directory.join("committed.json"))?)?;
            marker.validate()?;
            assert_eq!(marker.transaction_id, transaction_id);
            let state: TransactionStateRecord =
                serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
            assert_eq!(state.state, TransactionState::Applying);
            assert_eq!(state.applied_operations, vec![0, 1]);
            assert_eq!(fs::read(directory.join("backups/000000.json"))?, original_a);
            assert_eq!(fs::read(directory.join("backups/000001.json"))?, original_b);
            assert_eq!(fs::read(root.join("data/a.json"))?, staged_a);
            assert_eq!(fs::read(root.join("data/b.json"))?, staged_b);

            let report = recover_pending_transactions(project)?;
            assert_eq!(report.committed_cleanups, 1);
            assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new-a")?);
            assert_eq!(fs::read(root.join("data/b.json"))?, bytes("new-b")?);
            assert!(!directory.exists());
            assert!(recover_pending_transactions(project)?.nothing_to_recover());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn marker_identity_corruption_after_save_is_preserved_and_fails_closed() -> TestResult {
    with_project(|root, project| {
        let original = bytes("old")?;
        let staged = bytes("new")?;
        fs::write(root.join("data/a.json"), &original)?;
        let prepared = plan(&[("data/a.json", "new")])?.prepare_for_test(project)?;
        let directory = prepared.transaction_directory().to_path_buf();
        let marker_path = directory.join("committed.json");
        let foreign_marker = CommittedMarker {
            schema_version: TRANSACTION_SCHEMA_VERSION,
            transaction_id: TransactionId::parse(
                "txn-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            )?,
            completed_at_utc: super::super::utc_time::now_utc_milliseconds()?,
            project_fingerprint: project.fingerprint().to_owned(),
        };
        foreign_marker.validate()?;
        let hooks = Hooks {
            failure: CommitFailPoint::CommittedMarkerVerify,
            operation: None,
            mutation: RefCell::new(Some((
                marker_path.clone(),
                to_deterministic_json_bytes(&foreign_marker)?,
            ))),
        };

        let error = commit_with_hooks(prepared, &hooks)
            .expect_err("changed marker identity must require manual recovery");
        assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
        assert!(error.rollback_failure().is_none());
        assert_eq!(error.failure().stage, CommitStage::VerifyCommittedMarker);
        assert_eq!(error.failure().applied_operations(), &[0]);
        assert_eq!(
            fs::read(&marker_path)?,
            to_deterministic_json_bytes(&foreign_marker)?
        );
        let state: TransactionStateRecord =
            serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
        assert_eq!(state.state, TransactionState::Applying);
        assert_eq!(state.applied_operations, vec![0]);
        assert_eq!(fs::read(directory.join("backups/000000.json"))?, original);
        assert_eq!(fs::read(root.join("data/a.json"))?, staged);

        for _ in 0..2 {
            let recovery = recover_pending_transactions(project)
                .expect_err("foreign marker identity must remain fail closed");
            assert_eq!(
                recovery.result_state(),
                RecoveryResultState::ManualRecoveryRequired
            );
            assert!(directory.is_dir());
            assert_eq!(
                fs::read(&marker_path)?,
                to_deterministic_json_bytes(&foreign_marker)?
            );
            assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new")?);
            assert_eq!(
                fs::read(directory.join("backups/000000.json"))?,
                bytes("old")?
            );
        }
        Ok(())
    })
}

#[test]
fn marker_reread_type_failure_after_save_never_rolls_back() -> TestResult {
    with_project(|root, project| {
        let original = bytes("old")?;
        let target_path = root.join("data/a.json");
        fs::write(&target_path, &original)?;
        let prepared = plan(&[("data/a.json", "new")])?.prepare_for_test(project)?;
        let transaction_id = prepared.transaction_id().clone();
        let directory = prepared.transaction_directory().to_path_buf();
        let marker_path = directory.join("committed.json");
        let manifest_path = directory.join("manifest.json");
        let state_path = directory.join("state.json");
        let backup_path = directory.join("backups/000000.json");
        let staged_path = directory.join("staged/000000.json");

        let error = commit_with_hooks(
            prepared,
            &ReplaceMarkerWithDirectory {
                path: marker_path.clone(),
            },
        )
        .expect_err("unsafe marker type must stop after the commit decision");
        assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
        assert!(error.rollback_failure().is_none());
        assert_eq!(error.failure().transaction_id, transaction_id);
        assert_eq!(error.failure().stage, CommitStage::VerifyCommittedMarker);
        assert_eq!(error.failure().applied_operations(), &[0]);
        assert!(matches!(
            error.failure().source.as_ref(),
            CommitFailureSource::Io(_)
        ));
        assert!(marker_path.is_dir());
        let state: TransactionStateRecord = serde_json::from_slice(&fs::read(&state_path)?)?;
        assert_eq!(state.state, TransactionState::Applying);
        assert_eq!(state.applied_operations, vec![0]);
        assert_eq!(fs::read(&backup_path)?, original);
        assert_eq!(fs::read(&target_path)?, bytes("new")?);

        #[derive(Debug, PartialEq, Eq)]
        struct FilesystemSnapshot {
            transaction_entries: Vec<std::ffi::OsString>,
            marker_entries: Vec<std::ffi::OsString>,
            transaction_is_directory: bool,
            marker_is_directory: bool,
            marker_is_file: bool,
            marker_is_symlink: bool,
            rolled_back_marker_exists: bool,
            staged_artifact_exists: bool,
            manifest: Vec<u8>,
            state: Vec<u8>,
            backup: Vec<u8>,
            target: Vec<u8>,
        }

        // 반복 recovery가 rollback이나 cleanup을 시작하지 않았음을 journal topology와
        // 핵심 artifact/target bytes를 함께 비교해 고정한다.
        let snapshot = || -> io::Result<FilesystemSnapshot> {
            let mut transaction_entries = fs::read_dir(&directory)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<io::Result<Vec<_>>>()?;
            transaction_entries.sort();
            let mut marker_entries = fs::read_dir(&marker_path)?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<io::Result<Vec<_>>>()?;
            marker_entries.sort();
            let transaction_metadata = fs::symlink_metadata(&directory)?;
            let marker_metadata = fs::symlink_metadata(&marker_path)?;
            Ok(FilesystemSnapshot {
                transaction_entries,
                marker_entries,
                transaction_is_directory: transaction_metadata.is_dir(),
                marker_is_directory: marker_metadata.is_dir(),
                marker_is_file: marker_metadata.is_file(),
                marker_is_symlink: marker_metadata.file_type().is_symlink(),
                rolled_back_marker_exists: directory.join("rolled-back.json").exists(),
                staged_artifact_exists: staged_path.exists(),
                manifest: fs::read(&manifest_path)?,
                state: fs::read(&state_path)?,
                backup: fs::read(&backup_path)?,
                target: fs::read(&target_path)?,
            })
        };
        let before_recovery = snapshot()?;
        assert!(before_recovery.transaction_is_directory);
        assert!(before_recovery.marker_is_directory);
        assert!(!before_recovery.marker_is_file);
        assert!(!before_recovery.marker_is_symlink);
        assert!(!before_recovery.rolled_back_marker_exists);
        assert!(!before_recovery.staged_artifact_exists);

        let assert_manual_type_failure = |recovery: &RecoveryError| {
            assert_eq!(
                recovery.result_state(),
                RecoveryResultState::ManualRecoveryRequired
            );
            assert_eq!(
                recovery.failure().transaction_id.as_ref(),
                Some(&transaction_id)
            );
            assert_eq!(recovery.failure().stage, RecoveryStage::InspectJournal);
        };

        let first_recovery = recover_pending_transactions(project)
            .expect_err("unsafe marker type must remain fail closed on first recovery");
        assert_manual_type_failure(&first_recovery);
        let after_first_recovery = snapshot()?;
        assert_eq!(after_first_recovery, before_recovery);

        let second_recovery = recover_pending_transactions(project)
            .expect_err("unsafe marker type must remain fail closed on repeated recovery");
        assert_manual_type_failure(&second_recovery);
        let after_second_recovery = snapshot()?;
        assert_eq!(after_second_recovery, before_recovery);
        assert_eq!(after_second_recovery, after_first_recovery);
        Ok(())
    })
}

#[test]
fn marker_post_replace_failure_never_rolls_back_and_recovery_uses_marker_presence() -> TestResult {
    for preserve_marker in [true, false] {
        with_project(|root, project| {
            fs::write(root.join("data/a.json"), bytes("old-a")?)?;
            fs::write(root.join("data/b.json"), bytes("old-b")?)?;
            let prepared = plan(&[("data/a.json", "new-a"), ("data/b.json", "new-b")])?
                .prepare_for_test(project)?;
            let directory = prepared.transaction_directory().to_path_buf();

            let error = commit_with_hooks(
                prepared,
                &failing(CommitFailPoint::CommittedMarkerAfterReplace, None),
            )
            .expect_err("post-replace failure must require recovery");

            assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
            assert!(error.rollback_failure().is_none());
            match error.failure().source.as_ref() {
                CommitFailureSource::AtomicSave(SaveError::AtomicWrite(source)) => {
                    assert_eq!(source.outcome, SaveOutcome::AppliedDurabilityUncertain);
                }
                other => return Err(format!("unexpected marker failure source: {other:?}").into()),
            }
            assert!(directory.join("committed.json").is_file());
            let state: TransactionStateRecord =
                serde_json::from_slice(&fs::read(directory.join("state.json"))?)?;
            assert_eq!(state.state, TransactionState::Applying);
            assert_eq!(state.applied_operations, vec![0, 1]);
            assert_eq!(
                fs::read(directory.join("backups/000000.json"))?,
                bytes("old-a")?
            );
            assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new-a")?);
            assert_eq!(fs::read(root.join("data/b.json"))?, bytes("new-b")?);

            if !preserve_marker {
                // directory sync 전에 전원이 끊겨 marker directory entry가 유실된 경우를
                // 구성한다. marker가 없으면 기존 manifest 규칙에 따라 rollback한다.
                fs::remove_file(directory.join("committed.json"))?;
            }
            let report = recover_pending_transactions(project)?;
            if preserve_marker {
                assert_eq!(report.committed_cleanups, 1);
                assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new-a")?);
                assert_eq!(fs::read(root.join("data/b.json"))?, bytes("new-b")?);
            } else {
                assert_eq!(report.rolled_back_transactions, 1);
                assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
                assert_eq!(fs::read(root.join("data/b.json"))?, bytes("old-b")?);
            }
            assert!(!directory.exists());
            assert!(recover_pending_transactions(project)?.nothing_to_recover());
            if preserve_marker {
                assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new-a")?);
                assert_eq!(fs::read(root.join("data/b.json"))?, bytes("new-b")?);
            } else {
                assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
                assert_eq!(fs::read(root.join("data/b.json"))?, bytes("old-b")?);
            }
            Ok(())
        })?;
    }
    Ok(())
}

struct ApplyAndRollbackFail;

impl CommitHooks for ApplyAndRollbackFail {
    fn check(&self, point: CommitFailPoint, operation: Option<u32>) -> io::Result<()> {
        if point == CommitFailPoint::Rename && operation == Some(1) {
            Err(io::Error::other("injected apply failure"))
        } else {
            Ok(())
        }
    }

    fn check_recovery(&self, point: RecoveryFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == RecoveryFailPoint::RollingBackState {
            Err(io::Error::other("injected rollback failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn rollback_failure_preserves_apply_and_rollback_errors() -> TestResult {
    with_project(|root, project| {
        for name in ["a", "b"] {
            fs::write(root.join(format!("data/{name}.json")), bytes("old")?)?;
        }
        let prepared = plan(&[("data/a.json", "new-a"), ("data/b.json", "new-b")])?
            .prepare_for_test(project)?;
        let directory = prepared.transaction_directory().to_path_buf();
        let error = commit_with_hooks(prepared, &ApplyAndRollbackFail)
            .expect_err("rollback failure must require recovery");
        assert_eq!(error.result_state(), CommitResultState::RecoveryRequired);
        assert!(error.rollback_failure().is_some());
        assert!(directory.is_dir());
        assert!(directory.join("manifest.json").is_file());
        Ok(())
    })
}

#[test]
fn applying_failure_is_not_applied_and_committed_failures_stay_committed() -> TestResult {
    for point in [
        CommitFailPoint::ManifestRevalidation,
        CommitFailPoint::ApplyingState,
        CommitFailPoint::StagedRevalidation,
    ] {
        with_project(|root, project| {
            let target = root.join("data/a.json");
            let original = bytes("old")?;
            fs::write(&target, &original)?;
            let prepared = plan(&[("data/a.json", "new")])?.prepare_for_test(project)?;
            let error = commit_with_hooks(prepared, &failing(point, None))
                .expect_err("pre-apply failure must fail");
            assert_eq!(error.result_state(), CommitResultState::NotApplied);
            assert_eq!(fs::read(target)?, original);
            Ok(())
        })?;
    }

    for point in [CommitFailPoint::CommittedState, CommitFailPoint::Cleanup] {
        with_project(|root, project| {
            let prepared = plan(&[("data/a.json", "new")])?.prepare_for_test(project)?;
            let directory = prepared.transaction_directory().to_path_buf();
            let outcome = commit_with_hooks(prepared, &failing(point, None))?;
            assert_eq!(
                outcome.result_state(),
                CommitResultState::CommittedCleanupFailed
            );
            assert!(outcome.failures().is_some());
            assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new")?);
            if point == CommitFailPoint::Cleanup {
                let marker: CommittedMarker =
                    serde_json::from_slice(&fs::read(directory.join("committed.json"))?)?;
                marker.validate()?;
                assert!(super::super::utc_time::is_utc_milliseconds(
                    &marker.completed_at_utc
                ));
            }
            Ok(())
        })?;
    }
    Ok(())
}
