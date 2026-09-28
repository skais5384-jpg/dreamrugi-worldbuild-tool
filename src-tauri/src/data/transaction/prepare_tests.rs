use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use serde::{ser::Error as _, Serialize, Serializer};

use super::super::json::to_deterministic_json_bytes;
use super::super::project_lock::ProjectLock;
use super::super::storage_estimate::test_support::{with_storage_response, TestStorageResponse};
use super::super::storage_estimate::{
    StorageAdmissionDecision, StorageAdmissionError, StorageQueryErrorKind,
};
use super::prepare::{
    checked_estimate, prepare_with_hooks_for_test, PrepareFailPoint, PrepareHooks,
};
use super::*;
use crate::data::collaboration_lock::{
    LockCoordinator, LockService, LockSessionId, NoLockService, WritePermitError,
};

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Document<'a> {
    schema_version: u32,
    title: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    schema_version: u32,
    enabled: bool,
}

struct AlwaysFails;

impl Serialize for AlwaysFails {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Err(S::Error::custom(
            "project body secret must not be displayed",
        ))
    }
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "worldbuild-transaction-prepare-{}-{count}",
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

    fn project(&self) -> io::Result<PathBuf> {
        let path = self.0.join("project");
        fs::create_dir(&path)?;
        fs::create_dir(path.join("data"))?;
        Ok(path)
    }

    fn lock_root(&self) -> PathBuf {
        self.0.join("app-data")
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _cleanup_result = fs::remove_dir_all(&self.0);
    }
}

fn with_locked_project<F>(test: F) -> TestResult
where
    F: FnOnce(&TestDirectory, &Path, &LockedProject<'_>) -> TestResult,
{
    let temp = TestDirectory::new()?;
    let project_root = temp.project()?;
    let lock = ProjectLock::try_acquire(&project_root, &temp.lock_root())?;
    let project = LockedProject::bind(&lock, &project_root)?;
    test(&temp, &project_root, &project)
}

fn write_document(path: &Path, title: &str) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let bytes = to_deterministic_json_bytes(&Document {
        schema_version: 1,
        title,
    })?;
    fs::write(path, &bytes)?;
    Ok(bytes)
}

#[test]
fn prepares_multiple_existing_files_and_keeps_targets_unchanged() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let first_path = root.join("data/a.json");
        let second_path = root.join("data/b.json");
        let first_original = write_document(&first_path, "old-a")?;
        let second_original = write_document(&second_path, "old-b")?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/b.json",
            &Document {
                schema_version: 1,
                title: "new-b",
            },
        )?;
        plan.add_json(
            "data/a.json",
            &Document {
                schema_version: 1,
                title: "new-a",
            },
        )?;

        let prepared = plan.prepare_for_test(project)?;

        assert_eq!(fs::read(&first_path)?, first_original);
        assert_eq!(fs::read(&second_path)?, second_original);
        assert_eq!(
            prepared.manifest().operations[0].target_path.as_str(),
            "data/a.json"
        );
        assert_eq!(
            prepared.manifest().operations[1].target_path.as_str(),
            "data/b.json"
        );
        assert!(prepared
            .manifest()
            .operations
            .iter()
            .all(|operation| operation.original_existed));
        assert_prepared_layout(&prepared)?;
        assert_artifacts_match_manifest(&prepared)?;
        Ok(())
    })
}

#[test]
fn ordinary_existing_updates_reject_schema_increase_and_decrease_before_artifacts() -> TestResult {
    for (original_version, staged_version) in [(1, 2), (2, 1)] {
        with_locked_project(|_temp, root, project| {
            let target = root.join("data/existing.json");
            let original = to_deterministic_json_bytes(&Document {
                schema_version: original_version,
                title: "original",
            })?;
            fs::write(&target, &original)?;
            let mut plan = TransactionPlan::new();
            plan.add_json(
                "data/existing.json",
                &Document {
                    schema_version: staged_version,
                    title: "replacement",
                },
            )?;

            let error = plan
                .prepare_for_test(project)
                .err()
                .ok_or("ordinary schema transition must be rejected")?;
            assert!(error.schema_transition_mismatch());
            assert_eq!(fs::read(&target)?, original);
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn ordinary_same_schema_update_and_new_file_still_prepare() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let existing = root.join("data/existing.json");
        let original = write_document(&existing, "original")?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/existing.json",
            &Document {
                schema_version: 1,
                title: "replacement",
            },
        )?;
        plan.add_json(
            "data/new.json",
            &Document {
                schema_version: 2,
                title: "new file may choose its initial schema",
            },
        )?;

        let prepared = plan.prepare_for_test(project)?;
        assert_eq!(fs::read(existing)?, original);
        assert!(!root.join("data/new.json").exists());
        assert_eq!(prepared.manifest().operations.len(), 2);
        let outcome = prepared.commit()?;
        assert_eq!(outcome.result_state(), CommitResultState::Committed);
        assert_eq!(
            super::prepare::managed_schema_version(&fs::read(root.join("data/existing.json"))?)?
                .get(),
            1
        );
        assert_eq!(
            super::prepare::managed_schema_version(&fs::read(root.join("data/new.json"))?)?.get(),
            2
        );
        Ok(())
    })
}

#[test]
fn ordinary_storage_denial_query_errors_and_cross_volume_are_pre_artifact() -> TestResult {
    for response in [
        TestStorageResponse::Available {
            available_bytes: 0,
            allocation_unit_bytes: 4096,
        },
        TestStorageResponse::QueryFailure,
        TestStorageResponse::Unsupported,
        TestStorageResponse::CrossFilesystem,
    ] {
        with_locked_project(|_temp, root, project| {
            let target = root.join("data/existing.json");
            let original = write_document(&target, "original")?;
            let mut plan = TransactionPlan::new();
            plan.add_json(
                "data/existing.json",
                &Document {
                    schema_version: 1,
                    title: "replacement secret",
                },
            )?;

            let (result, records) =
                with_storage_response(response, || plan.prepare_for_test(project));
            let error = result.err().ok_or("storage admission must fail")?;
            match (response, error) {
                (
                    TestStorageResponse::Available { .. },
                    TransactionPrepareError::StorageAdmission(StorageAdmissionError::Insufficient(
                        admission,
                    )),
                ) => {
                    assert_eq!(admission.available_bytes(), 0);
                    assert_eq!(admission.decision(), StorageAdmissionDecision::Insufficient);
                }
                (
                    TestStorageResponse::QueryFailure,
                    TransactionPrepareError::StorageAdmission(source),
                ) => assert_eq!(source.kind(), Some(StorageQueryErrorKind::QueryFailed)),
                (
                    TestStorageResponse::Unsupported,
                    TransactionPrepareError::StorageAdmission(source),
                ) => assert_eq!(
                    source.kind(),
                    Some(StorageQueryErrorKind::UnsupportedPlatform)
                ),
                (
                    TestStorageResponse::CrossFilesystem,
                    TransactionPrepareError::StorageAdmission(source),
                ) => assert_eq!(source.kind(), Some(StorageQueryErrorKind::CrossFilesystem)),
                _ => return Err("unexpected storage admission error".into()),
            }
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].artifact_path, project.transactions_root());
            assert_eq!(
                records[0].target_paths,
                vec![project.canonical_root().join("data/existing.json")]
            );
            assert_eq!(fs::read(&target)?, original);
            assert!(!root.join(".worldbuild").exists());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn sufficient_storage_preserves_prepare_commit_and_safe_report() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let target = root.join("data/existing.json");
        write_document(&target, "original")?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/existing.json",
            &Document {
                schema_version: 1,
                title: "replacement",
            },
        )?;
        let (prepared, records) = with_storage_response(
            TestStorageResponse::Available {
                available_bytes: u64::MAX,
                allocation_unit_bytes: 4096,
            },
            || plan.prepare_for_test(project),
        );
        let prepared = prepared?;
        let admission = prepared.storage_admission();
        assert_eq!(admission.available_bytes(), u64::MAX);
        assert_eq!(admission.allocation_unit_bytes(), 4096);
        assert_eq!(admission.metadata_allowance_bytes(), 64 * 1024);
        assert!(admission.operational_reserve_bytes() >= 8 * 1024 * 1024);
        assert_eq!(admission.decision(), StorageAdmissionDecision::Admitted);
        assert_eq!(
            prepared.estimated_required_bytes(),
            admission.required_peak_bytes()
        );
        assert_eq!(records[0].artifact_path, project.transactions_root());
        assert_eq!(
            prepared.commit()?.result_state(),
            CommitResultState::Committed
        );
        assert_eq!(
            super::prepare::managed_schema_version(&fs::read(target)?)?.get(),
            1
        );
        Ok(())
    })
}

#[test]
fn prepares_multiple_new_files_without_backups() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/new-b.json",
            &Document {
                schema_version: 1,
                title: "new-b",
            },
        )?;
        plan.add_json(
            "data/new-a.json",
            &Document {
                schema_version: 99,
                title: "future schema remains a compatibility concern",
            },
        )?;

        let prepared = plan.prepare_for_test(project)?;

        assert!(!root.join("data/new-a.json").exists());
        assert!(!root.join("data/new-b.json").exists());
        assert!(prepared.manifest().operations.iter().all(|operation| {
            !operation.original_existed
                && operation.backup_path.is_none()
                && operation.original_size.is_none()
                && operation.original_sha256.is_none()
                && operation.original_schema_version.is_none()
                && operation.staged_schema_version.is_some()
        }));
        assert!(
            fs::read_dir(prepared.transaction_directory().join("backups"))?
                .next()
                .is_none()
        );
        assert_artifacts_match_manifest(&prepared)?;
        Ok(())
    })
}

#[test]
fn mixes_existing_new_and_different_rust_types() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let existing_path = root.join("data/existing.json");
        let original = write_document(&existing_path, "old")?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/existing.json",
            &Settings {
                schema_version: 1,
                enabled: true,
            },
        )?;
        plan.add_json(
            "data/new.json",
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;

        let prepared = plan.prepare_for_test(project)?;

        assert_eq!(fs::read(existing_path)?, original);
        assert!(!root.join("data/new.json").exists());
        assert_eq!(prepared.manifest().operations.len(), 2);
        Ok(())
    })
}

#[test]
fn rejects_empty_duplicate_and_serialization_failure_before_filesystem_creation() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let empty_error = TransactionPlan::new()
            .prepare_for_test(project)
            .err()
            .ok_or("empty plan unexpectedly prepared")?;
        assert!(matches!(empty_error, TransactionPrepareError::EmptyPlan));
        assert!(!root.join(".worldbuild").exists());

        let mut duplicate = TransactionPlan::new();
        duplicate.add_json(
            "data/file.json",
            &Document {
                schema_version: 1,
                title: "first",
            },
        )?;
        let duplicate_error = duplicate
            .add_json(
                r"data\file.json",
                &Document {
                    schema_version: 1,
                    title: "second",
                },
            )
            .expect_err("duplicate target should fail");
        assert!(matches!(
            duplicate_error,
            TransactionPrepareError::Model(TransactionModelError::DuplicateTarget { .. })
        ));
        assert!(!root.join(".worldbuild").exists());

        let mut serialization = TransactionPlan::new();
        let error = serialization
            .add_json("data/fail.json", &AlwaysFails)
            .expect_err("custom serialization should fail");
        assert!(!error.to_string().contains("project body secret"));
        assert!(!root.join(".worldbuild").exists());
        Ok(())
    })
}

#[test]
fn permit_project_and_target_mismatches_fail_before_filesystem_access() -> TestResult {
    for (mismatch_project, plan_targets, permit_targets) in [
        (true, &["data/item.json"][..], &["data/item.json"][..]),
        (
            false,
            &["data/item.json", "data/second.json"][..],
            &["data/item.json"][..],
        ),
        (
            false,
            &["data/item.json"][..],
            &["data/item.json", "data/extra.json"][..],
        ),
    ] {
        with_locked_project(|_temp, root, project| {
            let mut plan = TransactionPlan::new();
            for target in plan_targets {
                plan.add_json(
                    target,
                    &Document {
                        schema_version: 1,
                        title: "new",
                    },
                )?;
            }
            let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
            let coordinator = LockCoordinator::new(service);
            let session = LockSessionId::generate()?;
            let fingerprint = if mismatch_project {
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            } else {
                project.fingerprint()
            };
            let targets = permit_targets
                .iter()
                .map(|target| ProjectRelativePath::parse(target))
                .collect::<Result<Vec<_>, _>>()?;
            let mut guard = coordinator.acquire_all(fingerprint, &session, targets)?;
            let permit = guard.write_permit()?;
            let error = plan
                .prepare(project, permit)
                .err()
                .ok_or("mismatched permit unexpectedly prepared")?;
            assert!(matches!(
                error,
                TransactionPrepareError::Permit(WritePermitError::ProjectMismatch)
                    | TransactionPrepareError::Permit(WritePermitError::TargetMismatch)
            ));
            assert!(!root.join(".worldbuild").exists());
            assert!(!root.join("data/item.json").exists());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn rejects_missing_parent_and_non_regular_target_before_transaction_creation() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let mut missing = TransactionPlan::new();
        missing.add_json(
            "missing/file.json",
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;
        assert!(missing.prepare_for_test(project).is_err());
        assert!(!root.join(".worldbuild").exists());

        fs::create_dir(root.join("data/directory.json"))?;
        let mut directory = TransactionPlan::new();
        directory.add_json(
            "data/directory.json",
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;
        assert!(directory.prepare_for_test(project).is_err());
        assert!(!root.join(".worldbuild").exists());
        Ok(())
    })
}

#[cfg(unix)]
#[test]
fn rejects_parent_outside_project_and_symbolic_link_target() -> TestResult {
    use std::os::unix::fs::symlink;
    with_locked_project(|temp, root, project| {
        let outside = temp.0.join("outside");
        fs::create_dir(&outside)?;
        symlink(&outside, root.join("linked-parent"))?;
        let mut outside_plan = TransactionPlan::new();
        outside_plan.add_json(
            "linked-parent/file.json",
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;
        assert!(outside_plan.prepare_for_test(project).is_err());

        let outside_file = outside.join("source.json");
        write_document(&outside_file, "outside")?;
        symlink(&outside_file, root.join("data/link.json"))?;
        let mut link_plan = TransactionPlan::new();
        link_plan.add_json(
            "data/link.json",
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;
        assert!(link_plan.prepare_for_test(project).is_err());
        assert!(!root.join(".worldbuild").exists());
        Ok(())
    })
}

#[test]
fn staged_backup_manifest_and_state_are_exact_and_valid() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let target = root.join("data/item.json");
        let original = write_document(&target, "original")?;
        let replacement = Document {
            schema_version: 1,
            title: "replacement",
        };
        let expected_staged = to_deterministic_json_bytes(&replacement)?;
        let mut plan = TransactionPlan::new();
        plan.add_json("data/item.json", &replacement)?;

        let prepared = plan.prepare_for_test(project)?;
        let operation = &prepared.manifest().operations[0];
        let staged = fs::read(
            prepared
                .transaction_directory()
                .join(&operation.staged_path),
        )?;
        let backup = fs::read(
            prepared.transaction_directory().join(
                operation
                    .backup_path
                    .as_deref()
                    .ok_or("backup path missing")?,
            ),
        )?;
        let manifest_bytes = fs::read(prepared.transaction_directory().join("manifest.json"))?;
        let decoded_manifest: TransactionManifest = serde_json::from_slice(&manifest_bytes)?;
        let state: TransactionStateRecord = serde_json::from_slice(&fs::read(
            prepared.transaction_directory().join("state.json"),
        )?)?;

        assert_eq!(staged, expected_staged);
        assert_eq!(backup, original);
        assert_eq!(operation.staged_size, staged.len() as u64);
        assert_eq!(operation.original_size, Some(backup.len() as u64));
        assert!(operation.staged_schema_version.is_some());
        assert!(operation.original_schema_version.is_some());
        assert_eq!(
            manifest_bytes,
            to_deterministic_json_bytes(prepared.manifest())?
        );
        decoded_manifest.validate()?;
        state.validate()?;
        assert_eq!(state.state, TransactionState::Prepared);
        assert!(state.applied_operations.is_empty());
        assert_eq!(state.transaction_id, *prepared.transaction_id());
        assert_eq!(prepared.project_fingerprint(), project.fingerprint());
        Ok(())
    })
}

#[test]
fn prepared_transaction_drop_keeps_recovery_material() -> TestResult {
    with_locked_project(|_temp, _root, project| {
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/new.json",
            &Document {
                schema_version: 1,
                title: "new",
            },
        )?;
        let prepared = plan.prepare_for_test(project)?;
        let directory = prepared.transaction_directory().to_path_buf();
        drop(prepared);

        assert!(directory.join("state.json").is_file());
        assert!(directory.join("manifest.json").is_file());
        Ok(())
    })
}

#[test]
fn estimates_required_bytes_and_detects_overflow() -> TestResult {
    let mut plan = TransactionPlan::new();
    plan.add_json(
        "data/file.json",
        &Document {
            schema_version: 1,
            title: "estimate",
        },
    )?;
    assert!(plan.estimated_staged_bytes()? > 0);
    assert!(matches!(
        checked_estimate(u64::MAX, 1, 0, 0, 0),
        Err(TransactionPrepareError::EstimateOverflow)
    ));

    with_locked_project(|_temp, _root, project| {
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/file.json",
            &Document {
                schema_version: 1,
                title: "estimate",
            },
        )?;
        let staged = plan.estimated_staged_bytes()?;
        let prepared = plan.prepare_for_test(project)?;
        assert!(prepared.estimated_required_bytes() > staged);
        Ok(())
    })
}

#[derive(Clone, Copy)]
struct InjectedHooks {
    failure: PrepareFailPoint,
    cleanup_failure: bool,
}

struct SchemaRaceHook {
    path: PathBuf,
    replacement: Vec<u8>,
}

impl PrepareHooks for SchemaRaceHook {
    fn check(&self, point: PrepareFailPoint, operation: Option<u32>) -> io::Result<()> {
        if point == PrepareFailPoint::BeforeOriginalRead && operation == Some(0) {
            fs::write(&self.path, &self.replacement)?;
        }
        Ok(())
    }
}

#[test]
fn backup_handle_read_rechecks_ordinary_schema_after_precheck_race() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let target = root.join("data/existing.json");
        write_document(&target, "original")?;
        let replacement = to_deterministic_json_bytes(&Document {
            schema_version: 2,
            title: "external writer",
        })?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/existing.json",
            &Document {
                schema_version: 1,
                title: "planned",
            },
        )?;

        let error = prepare_with_hooks_for_test(
            plan,
            project,
            &SchemaRaceHook {
                path: target.clone(),
                replacement: replacement.clone(),
            },
        )
        .err()
        .ok_or("schema race before backup must be rejected")?;
        assert!(error.schema_transition_mismatch());
        assert_eq!(fs::read(target)?, replacement);
        assert_transactions_empty(root)?;
        Ok(())
    })
}

impl PrepareHooks for InjectedHooks {
    fn check(&self, point: PrepareFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == PrepareFailPoint::Cleanup && self.cleanup_failure {
            return Err(io::Error::other("injected cleanup failure"));
        }
        if point == self.failure {
            return Err(io::Error::other(format!("injected {point:?} failure")));
        }
        Ok(())
    }
}

#[test]
fn every_injected_prepare_failure_preserves_targets_and_cleans_transaction() -> TestResult {
    for failure in [
        PrepareFailPoint::CreateTransactionDirectory,
        PrepareFailPoint::TransactionDirectoryAllocated,
        PrepareFailPoint::PreparingState,
        PrepareFailPoint::BackupCreate,
        PrepareFailPoint::BackupWrite,
        PrepareFailPoint::BackupSync,
        PrepareFailPoint::StagedCreate,
        PrepareFailPoint::StagedWrite,
        PrepareFailPoint::StagedSync,
        PrepareFailPoint::ManifestCreate,
        PrepareFailPoint::ManifestSync,
        PrepareFailPoint::ManifestVerify,
        PrepareFailPoint::PreparedState,
    ] {
        with_locked_project(|_temp, root, project| {
            let target = root.join("data/existing.json");
            let original = write_document(&target, "original")?;
            let mut plan = TransactionPlan::new();
            plan.add_json(
                "data/existing.json",
                &Document {
                    schema_version: 1,
                    title: "replacement",
                },
            )?;
            let result = prepare_with_hooks_for_test(
                plan,
                project,
                &InjectedHooks {
                    failure,
                    cleanup_failure: false,
                },
            );

            assert!(result.is_err(), "{failure:?}");
            assert_eq!(fs::read(target)?, original, "{failure:?}");
            assert_transactions_empty(root)?;
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn cleanup_failure_preserves_primary_error_and_recovery_identity() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let target = root.join("data/existing.json");
        let original = write_document(&target, "original")?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/existing.json",
            &Document {
                schema_version: 1,
                title: "replacement project body",
            },
        )?;

        let error = prepare_with_hooks_for_test(
            plan,
            project,
            &InjectedHooks {
                failure: PrepareFailPoint::StagedWrite,
                cleanup_failure: true,
            },
        )
        .err()
        .ok_or("injected failure unexpectedly succeeded")?;

        assert!(error.cleanup_error().is_some());
        assert!(std::error::Error::source(&error).is_some());
        assert_eq!(fs::read(target)?, original);
        assert!(!error.to_string().contains(&root.display().to_string()));
        assert!(!error.to_string().contains("replacement project body"));
        assert!(fs::read_dir(root.join(".worldbuild/transactions"))?
            .next()
            .is_some());
        Ok(())
    })
}

#[test]
fn invalid_existing_json_fails_without_changing_target_and_cleans_transaction() -> TestResult {
    with_locked_project(|_temp, root, project| {
        let target = root.join("data/invalid.json");
        let original = b"{\"title\":\"missing schema\"}\n".to_vec();
        fs::write(&target, &original)?;
        let mut plan = TransactionPlan::new();
        plan.add_json(
            "data/invalid.json",
            &Document {
                schema_version: 1,
                title: "replacement",
            },
        )?;

        assert!(plan.prepare_for_test(project).is_err());
        assert_eq!(fs::read(target)?, original);
        assert_transactions_empty(root)?;
        Ok(())
    })
}

fn assert_prepared_layout(prepared: &PreparedTransaction<'_, '_, '_>) -> TestResult {
    let directory = prepared.transaction_directory();
    assert!(directory.join("state.json").is_file());
    assert!(directory.join("manifest.json").is_file());
    assert!(directory.join("staged").is_dir());
    assert!(directory.join("backups").is_dir());
    assert_eq!(
        directory.parent().and_then(Path::file_name),
        Some(std::ffi::OsStr::new("transactions"))
    );
    assert!(OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join("manifest.json"))
        .is_err());
    Ok(())
}

fn assert_artifacts_match_manifest(prepared: &PreparedTransaction<'_, '_, '_>) -> TestResult {
    for operation in &prepared.manifest().operations {
        let staged = fs::read(
            prepared
                .transaction_directory()
                .join(&operation.staged_path),
        )?;
        assert_eq!(staged.len() as u64, operation.staged_size);
        let _: serde_json::Value = serde_json::from_slice(&staged)?;
        let _: super::super::schema::SchemaHeader = serde_json::from_slice(&staged)?;
        if let Some(backup_path) = &operation.backup_path {
            let backup = fs::read(prepared.transaction_directory().join(backup_path))?;
            assert_eq!(Some(backup.len() as u64), operation.original_size);
            let _: serde_json::Value = serde_json::from_slice(&backup)?;
            let _: super::super::schema::SchemaHeader = serde_json::from_slice(&backup)?;
        }
    }
    Ok(())
}

fn assert_transactions_empty(root: &Path) -> io::Result<()> {
    let transactions = root.join(".worldbuild/transactions");
    if !transactions.exists() {
        return Ok(());
    }
    if fs::read_dir(transactions)?.next().is_some() {
        return Err(io::Error::other("transaction directory was not cleaned"));
    }
    Ok(())
}
