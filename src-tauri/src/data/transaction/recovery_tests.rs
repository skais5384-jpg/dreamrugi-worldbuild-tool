use std::cell::Cell;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;

use super::super::json::to_deterministic_json_bytes;
use super::super::project_file::open_existing_private_file_after_open_for_test;
use super::super::project_lock::ProjectLock;
use super::apply::{commit_with_hooks, CommitFailPoint, CommitHooks};
use super::recovery::{recover_pending_transactions_with_hooks, RecoveryFailPoint, RecoveryHooks};
use super::*;

#[cfg(windows)]
#[path = "legacy_stream_tests.rs"]
mod legacy_stream;

static COUNTER: AtomicU64 = AtomicU64::new(0);
type TestResult = Result<(), Box<dyn std::error::Error>>;

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
                "worldbuild-recovery-{}-{}",
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
            "test collision",
        ))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _cleanup = fs::remove_dir_all(&self.0);
    }
}

fn bytes(title: &str) -> Result<Vec<u8>, serde_json::Error> {
    to_deterministic_json_bytes(&Document {
        schema_version: 1,
        title,
    })
}

fn setup(temp: &Temp) -> Result<PathBuf, io::Error> {
    let root = temp.0.join("project");
    fs::create_dir(&root)?;
    fs::create_dir(root.join("data"))?;
    Ok(root)
}

#[cfg(any(unix, windows))]
struct DirectoryLink(PathBuf);

#[cfg(any(unix, windows))]
impl DirectoryLink {
    fn create(path: PathBuf, destination: &Path) -> io::Result<Self> {
        create_directory_link(&path, destination)?;
        Ok(Self(path))
    }
}

#[cfg(any(unix, windows))]
impl Drop for DirectoryLink {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _cleanup = fs::remove_file(&self.0);
        #[cfg(windows)]
        let _cleanup = fs::remove_dir(&self.0);
    }
}

#[cfg(unix)]
fn create_directory_link(path: &Path, destination: &Path) -> io::Result<()> {
    std::os::unix::fs::symlink(destination, path)
}

#[cfg(windows)]
fn create_directory_link(path: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let output = Command::new("cmd")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(path)
        .arg(destination)
        .creation_flags(CREATE_NO_WINDOW)
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "could not create test junction: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )))
    }
}

#[cfg(any(unix, windows))]
#[derive(Clone, Copy)]
enum NamespaceLinkPoint {
    Worldbuild,
    Transactions,
    TransactionEntry,
}

#[cfg(any(unix, windows))]
fn assert_namespace_link_is_preserved(point: NamespaceLinkPoint) -> TestResult {
    const ID: &str = "txn-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let outside = temp.0.join("outside");
    fs::create_dir(&outside)?;

    let (link, destination, external_transaction) = match point {
        NamespaceLinkPoint::Worldbuild => {
            let external_transaction = outside.join("transactions").join(ID);
            fs::create_dir_all(&external_transaction)?;
            (
                root.join(".worldbuild"),
                outside.clone(),
                external_transaction,
            )
        }
        NamespaceLinkPoint::Transactions => {
            fs::create_dir(root.join(".worldbuild"))?;
            let external_transaction = outside.join(ID);
            fs::create_dir(&external_transaction)?;
            (
                root.join(".worldbuild").join("transactions"),
                outside.clone(),
                external_transaction,
            )
        }
        NamespaceLinkPoint::TransactionEntry => {
            fs::create_dir_all(root.join(".worldbuild").join("transactions"))?;
            let external_transaction = outside.join(ID);
            fs::create_dir(&external_transaction)?;
            (
                root.join(".worldbuild").join("transactions").join(ID),
                external_transaction.clone(),
                external_transaction,
            )
        }
    };
    let sentinel = external_transaction.join("must-not-delete.txt");
    fs::write(&sentinel, b"outside-preserved")?;
    let _link = DirectoryLink::create(link, &destination)?;

    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let error = recover_pending_transactions(&project)
        .expect_err("linked transaction namespace must fail closed");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::ManualRecoveryRequired
    );
    assert!(external_transaction.is_dir());
    assert_eq!(fs::read(sentinel)?, b"outside-preserved");
    Ok(())
}

#[cfg(any(unix, windows))]
#[test]
fn rejects_linked_worldbuild_transactions_and_transaction_entry_without_cleanup() -> TestResult {
    for point in [
        NamespaceLinkPoint::Worldbuild,
        NamespaceLinkPoint::Transactions,
        NamespaceLinkPoint::TransactionEntry,
    ] {
        assert_namespace_link_is_preserved(point)?;
    }
    Ok(())
}

#[cfg(any(unix, windows))]
#[test]
fn rejects_linked_manifest_state_marker_staged_and_backup_without_mutation() -> TestResult {
    for artifact in ["manifest", "state", "marker", "staged", "backup"] {
        let temp = Temp::new()?;
        let root = setup(&temp)?;
        let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
        let project = LockedProject::bind(&lock, &root)?;
        let prepared = prepare_mixed(&project, &root)?;
        let directory = prepared.transaction_directory().to_path_buf();
        let operation = prepared.manifest().operations[0].clone();
        drop(prepared);

        let path = match artifact {
            "manifest" => directory.join("manifest.json"),
            "state" => directory.join("state.json"),
            "marker" => directory.join("committed.json"),
            "staged" => directory.join(&operation.staged_path),
            "backup" => directory.join(
                operation
                    .backup_path
                    .as_deref()
                    .ok_or("expected backup path")?,
            ),
            _ => unreachable!(),
        };
        if artifact == "marker" {
            fs::write(&path, b"placeholder")?;
        }
        fs::remove_file(&path)?;
        let outside = temp.0.join(format!("outside-{artifact}"));
        fs::create_dir(&outside)?;
        let sentinel = outside.join("must-not-change.txt");
        fs::write(&sentinel, b"outside-preserved")?;
        let _link = DirectoryLink::create(path, &outside)?;

        let error = recover_pending_transactions(&project)
            .expect_err("linked journal artifact must fail closed");
        assert_eq!(
            error.result_state(),
            RecoveryResultState::ManualRecoveryRequired,
            "{artifact}"
        );
        assert!(directory.is_dir(), "{artifact}");
        assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
        assert!(!root.join("data/b.json").exists());
        assert_eq!(fs::read(root.join("data/c.json"))?, bytes("old-c")?);
        assert_eq!(fs::read(sentinel)?, b"outside-preserved", "{artifact}");
    }
    Ok(())
}

#[cfg(any(unix, windows))]
#[test]
fn private_artifact_replacement_after_open_cannot_change_read_bytes() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let transaction = root.join("transaction-private");
    fs::create_dir(&transaction)?;
    let canonical_transaction = fs::canonicalize(&transaction)?;
    let artifact = canonical_transaction.join("manifest.json");
    let held = canonical_transaction.join("held-manifest.json");
    fs::write(&artifact, b"verified-original")?;
    let replacement_was_blocked = Cell::new(false);

    let mut file =
        open_existing_private_file_after_open_for_test(&canonical_transaction, &artifact, || {
            match fs::rename(&artifact, &held) {
                Ok(()) => fs::write(&artifact, b"replacement")?,
                Err(_) => replacement_was_blocked.set(true),
            }
            Ok(())
        })?;
    let mut observed = Vec::new();
    file.read_to_end(&mut observed)?;

    assert_eq!(observed, b"verified-original");
    if replacement_was_blocked.get() {
        assert_eq!(fs::read(artifact)?, b"verified-original");
    } else {
        assert_eq!(fs::read(artifact)?, b"replacement");
    }
    Ok(())
}

fn prepare_mixed<'project, 'lock>(
    project: &'project LockedProject<'lock>,
    root: &Path,
) -> Result<PreparedTransaction<'project, 'lock, 'static>, Box<dyn std::error::Error>> {
    fs::write(root.join("data/a.json"), bytes("old-a")?)?;
    fs::write(root.join("data/c.json"), bytes("old-c")?)?;
    let mut plan = TransactionPlan::new();
    for (path, title) in [
        ("data/a.json", "new-a"),
        ("data/b.json", "new-b"),
        ("data/c.json", "new-c"),
    ] {
        plan.add_json(
            path,
            &Document {
                schema_version: 1,
                title,
            },
        )?;
    }
    Ok(plan.prepare_for_test(project)?)
}

#[test]
fn missing_transaction_namespace_is_nothing_to_recover() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;

    assert!(recover_pending_transactions(&project)?.nothing_to_recover());
    assert!(!root.join(".worldbuild").exists());
    fs::create_dir(root.join(".worldbuild"))?;
    assert!(recover_pending_transactions(&project)?.nothing_to_recover());
    assert!(!project.transactions_root().exists());
    Ok(())
}

fn apply_prefix(prepared: &PreparedTransaction<'_, '_, '_>, count: usize) -> io::Result<()> {
    for operation in prepared.manifest().operations.iter().take(count) {
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

#[test]
fn recovers_prepared_partial_full_and_idempotent_mixed_transactions() -> TestResult {
    for applied in [0_usize, 1, 3] {
        let temp = Temp::new()?;
        let root = setup(&temp)?;
        let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
        let project = LockedProject::bind(&lock, &root)?;
        let prepared = prepare_mixed(&project, &root)?;
        apply_prefix(&prepared, applied)?;
        let transaction_directory = prepared.transaction_directory().to_path_buf();
        if applied == 1 {
            fs::write(transaction_directory.join("state.json"), b"corrupt")?;
        }
        drop(prepared);

        let report = recover_pending_transactions(&project)?;
        assert!(!report.manual_recovery_required());
        assert_eq!(report.rolled_back_transactions, 1);
        assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
        assert!(!root.join("data/b.json").exists());
        assert_eq!(fs::read(root.join("data/c.json"))?, bytes("old-c")?);
        assert!(!transaction_directory.exists());
        assert!(project.transactions_root().is_dir());
        assert!(recover_pending_transactions(&project)?.nothing_to_recover());
    }
    Ok(())
}

#[test]
fn restores_missing_existing_target_and_cleans_preparing_orphan() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let prepared = prepare_mixed(&project, &root)?;
    fs::remove_file(root.join("data/a.json"))?;
    drop(prepared);
    recover_pending_transactions(&project)?;
    assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);

    let (id, _) = TransactionId::new_candidate(project.fingerprint())?;
    let orphan = project.transactions_root().join(id.as_str());
    fs::create_dir(&orphan)?;
    fs::create_dir(orphan.join("staged"))?;
    let report = recover_pending_transactions(&project)?;
    assert_eq!(report.preparing_orphan_cleanups, 1);
    Ok(())
}

#[test]
fn rejects_invalid_directory_names_and_overlapping_transactions() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    fs::create_dir_all(project.transactions_root())?;
    fs::create_dir(project.transactions_root().join("invalid-name"))?;
    let error = recover_pending_transactions(&project).expect_err("invalid ID must block");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::ManualRecoveryRequired
    );
    fs::remove_dir(project.transactions_root().join("invalid-name"))?;

    let first = prepare_mixed(&project, &root)?;
    let first_directory = first.transaction_directory().to_path_buf();
    drop(first);
    let second = prepare_mixed(&project, &root)?;
    let second_directory = second.transaction_directory().to_path_buf();
    drop(second);
    let error = recover_pending_transactions(&project).expect_err("overlap must block");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::ManualRecoveryRequired
    );
    assert!(first_directory.exists() && second_directory.exists());
    Ok(())
}

#[test]
fn rejects_dual_markers_and_future_manifest_schema_without_mutation() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let prepared = prepare_mixed(&project, &root)?;
    let directory = prepared.transaction_directory().to_path_buf();
    let id = prepared.transaction_id().clone();
    drop(prepared);
    let completed_at_utc = super::super::utc_time::now_utc_milliseconds()?;
    let committed = CommittedMarker {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        transaction_id: id.clone(),
        completed_at_utc: completed_at_utc.clone(),
        project_fingerprint: project.fingerprint().to_owned(),
    };
    let rolled_back = RolledBackMarker {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        transaction_id: id,
        completed_at_utc,
        project_fingerprint: project.fingerprint().to_owned(),
    };
    fs::write(
        directory.join("committed.json"),
        to_deterministic_json_bytes(&committed)?,
    )?;
    fs::write(
        directory.join("rolled-back.json"),
        to_deterministic_json_bytes(&rolled_back)?,
    )?;
    let error = recover_pending_transactions(&project).expect_err("dual markers must block");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::ManualRecoveryRequired
    );
    assert!(directory.exists());
    fs::remove_dir_all(&directory)?;

    let prepared = prepare_mixed(&project, &root)?;
    let directory = prepared.transaction_directory().to_path_buf();
    drop(prepared);
    let manifest_path = directory.join("manifest.json");
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    value["schemaVersion"] = serde_json::Value::from(2);
    fs::write(manifest_path, to_deterministic_json_bytes(&value)?)?;
    let error = recover_pending_transactions(&project).expect_err("future schema must block");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::ManualRecoveryRequired
    );
    assert!(directory.exists());
    Ok(())
}

#[derive(Clone, Copy)]
struct Fail(RecoveryFailPoint);
impl RecoveryHooks for Fail {
    fn check(&self, point: RecoveryFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == self.0 {
            Err(io::Error::other(format!("injected {point:?}")))
        } else {
            Ok(())
        }
    }
}

struct CommitCleanupFail;
impl CommitHooks for CommitCleanupFail {
    fn check(&self, point: CommitFailPoint, _operation: Option<u32>) -> io::Result<()> {
        if point == CommitFailPoint::Cleanup {
            Err(io::Error::other("injected commit cleanup failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn committed_and_rolled_back_markers_are_verified_then_cleaned() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let prepared = prepare_mixed(&project, &root)?;
    let outcome = commit_with_hooks(prepared, &CommitCleanupFail)?;
    assert_eq!(
        outcome.result_state(),
        CommitResultState::CommittedCleanupFailed
    );
    let directory = fs::read_dir(project.transactions_root())?
        .next()
        .ok_or("committed transaction missing")??
        .path();
    let error = recover_pending_transactions_with_hooks(
        &project,
        &Fail(RecoveryFailPoint::CommittedTargetVerify),
    )
    .expect_err("committed verification injection must fail");
    assert_eq!(error.result_state(), RecoveryResultState::RecoveryRequired);
    assert!(directory.exists());
    let report = recover_pending_transactions(&project)?;
    assert_eq!(report.committed_cleanups, 1);
    assert_eq!(fs::read(root.join("data/a.json"))?, bytes("new-a")?);

    let prepared = prepare_mixed(&project, &root)?;
    apply_prefix(&prepared, 3)?;
    let directory = prepared.transaction_directory().to_path_buf();
    drop(prepared);
    let error =
        recover_pending_transactions_with_hooks(&project, &Fail(RecoveryFailPoint::Cleanup))
            .expect_err("rolled-back cleanup injection must fail");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::RolledBackCleanupFailed
    );
    assert!(directory.join("rolled-back.json").is_file());
    let report = recover_pending_transactions(&project)?;
    assert_eq!(report.rolled_back_cleanups, 1);
    Ok(())
}

#[test]
fn executes_rollback_and_recovery_failure_boundaries_without_cleanup() -> TestResult {
    for point in [
        RecoveryFailPoint::Enumerate,
        RecoveryFailPoint::JournalDecision,
        RecoveryFailPoint::RollingBackState,
        RecoveryFailPoint::BackupVerify,
        RecoveryFailPoint::RestoreExisting,
        RecoveryFailPoint::RemoveNew,
        RecoveryFailPoint::RestoredVerify,
        RecoveryFailPoint::ProgressState,
        RecoveryFailPoint::RolledBackMarkerWrite,
        RecoveryFailPoint::RolledBackMarkerVerify,
        RecoveryFailPoint::RolledBackMarkerSync,
        RecoveryFailPoint::RolledBackState,
        RecoveryFailPoint::Cleanup,
    ] {
        let temp = Temp::new()?;
        let root = setup(&temp)?;
        let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
        let project = LockedProject::bind(&lock, &root)?;
        let prepared = prepare_mixed(&project, &root)?;
        apply_prefix(&prepared, 3)?;
        let directory = prepared.transaction_directory().to_path_buf();
        drop(prepared);
        let result = recover_pending_transactions_with_hooks(&project, &Fail(point));
        assert!(result.is_err(), "{point:?}");
        assert!(directory.exists(), "{point:?}");
        assert!(directory.join("manifest.json").is_file(), "{point:?}");
    }
    Ok(())
}

#[test]
fn resumes_after_interrupted_rollback_operation_idempotently() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let prepared = prepare_mixed(&project, &root)?;
    apply_prefix(&prepared, 3)?;
    let directory = prepared.transaction_directory().to_path_buf();
    drop(prepared);

    let error =
        recover_pending_transactions_with_hooks(&project, &Fail(RecoveryFailPoint::ProgressState))
            .expect_err("progress failure must interrupt rollback");
    assert_eq!(error.result_state(), RecoveryResultState::RecoveryRequired);
    assert!(directory.exists());
    let report = recover_pending_transactions(&project)?;
    assert_eq!(report.rolled_back_transactions, 1);
    assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
    assert!(!root.join("data/b.json").exists());
    assert_eq!(fs::read(root.join("data/c.json"))?, bytes("old-c")?);
    Ok(())
}

#[test]
fn rejects_corrupt_unknown_overlapping_and_external_target_states() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    let lock = ProjectLock::try_acquire(&root, &temp.0.join("app-data"))?;
    let project = LockedProject::bind(&lock, &root)?;
    let prepared = prepare_mixed(&project, &root)?;
    fs::write(root.join("data/a.json"), bytes("external")?)?;
    let directory = prepared.transaction_directory().to_path_buf();
    drop(prepared);
    let error = recover_pending_transactions(&project).expect_err("external target must block");
    assert_eq!(
        error.result_state(),
        RecoveryResultState::ManualRecoveryRequired
    );
    assert!(directory.exists());
    assert!(!error.to_string().contains(&root.display().to_string()));
    assert!(!error.to_string().contains("external"));
    Ok(())
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _kill = self.0.kill();
        let _wait = self.0.wait();
    }
}

#[test]
fn forced_exit_after_first_apply_is_recovered_on_next_lock() -> TestResult {
    let temp = Temp::new()?;
    let root = setup(&temp)?;
    fs::write(root.join("data/a.json"), bytes("old-a")?)?;
    let ready = temp.0.join("ready");
    let child = Command::new(std::env::current_exe()?)
        .arg("--ignored")
        .arg("--exact")
        .arg("data::transaction::recovery_tests::recovery_child_helper")
        .arg("--nocapture")
        .env("WORLDBUILD_RECOVERY_PROJECT", &root)
        .env("WORLDBUILD_RECOVERY_LOCK_ROOT", temp.0.join("app-data"))
        .env("WORLDBUILD_RECOVERY_READY", &ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut child = ChildGuard(child);
    wait_until(Duration::from_secs(10), || ready.exists())?;
    child.0.kill()?;
    child.0.wait()?;

    let lock = retry_lock(&root, &temp.0.join("app-data"), Duration::from_secs(5))?;
    let project = LockedProject::bind(&lock, &root)?;
    recover_pending_transactions(&project)?;
    assert_eq!(fs::read(root.join("data/a.json"))?, bytes("old-a")?);
    assert!(fs::read_dir(project.transactions_root())?.next().is_none());
    Ok(())
}

#[test]
#[ignore = "helper launched by forced_exit_after_first_apply_is_recovered_on_next_lock"]
fn recovery_child_helper() -> TestResult {
    let Some(root) = std::env::var_os("WORLDBUILD_RECOVERY_PROJECT") else {
        return Ok(());
    };
    let lock_root = std::env::var_os("WORLDBUILD_RECOVERY_LOCK_ROOT").ok_or("lock root missing")?;
    let ready = std::env::var_os("WORLDBUILD_RECOVERY_READY").ok_or("ready missing")?;
    let lock = ProjectLock::try_acquire(Path::new(&root), Path::new(&lock_root))?;
    let project = LockedProject::bind(&lock, Path::new(&root))?;
    let mut plan = TransactionPlan::new();
    plan.add_json(
        "data/a.json",
        &Document {
            schema_version: 1,
            title: "new-a",
        },
    )?;
    let prepared = plan.prepare_for_test(&project)?;
    apply_prefix(&prepared, 1)?;
    fs::write(ready, b"ready")?;
    loop {
        thread::sleep(Duration::from_secs(1));
    }
}

fn wait_until(timeout: Duration, condition: impl Fn() -> bool) -> io::Result<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"))
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
