use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::super::json::to_deterministic_json_bytes;
use super::super::project_lock::ProjectLock;
use super::super::schema::SchemaVersion;
use super::*;

static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);
const HASH_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HASH_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const VALID_TIME: &str = "2026-08-31T01:23:45.678Z";

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> io::Result<Self> {
        for _ in 0..128 {
            let count = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "worldbuild-transaction-model-{}-{count}",
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

    fn project(&self, name: &str) -> io::Result<PathBuf> {
        let path = self.0.join(name);
        fs::create_dir(&path)?;
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

#[test]
fn binds_matching_project_lock_without_exposing_root() -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDirectory::new()?;
    let project = temp.project("project")?;
    let lock = ProjectLock::try_acquire(&project, &temp.lock_root())?;

    let bound = LockedProject::bind(&lock, &project)?;

    assert_eq!(bound.fingerprint(), lock.fingerprint());
    assert_eq!(bound.canonical_root(), fs::canonicalize(&project)?);
    assert_eq!(
        bound.transactions_root(),
        fs::canonicalize(&project)?
            .join(".worldbuild")
            .join("transactions")
    );
    Ok(())
}

#[test]
fn rejects_lock_bound_to_a_different_project_without_path_leak(
) -> Result<(), Box<dyn std::error::Error>> {
    let temp = TestDirectory::new()?;
    let first = temp.project("SECRET_FIRST_PROJECT")?;
    let second = temp.project("SECRET_SECOND_PROJECT")?;
    let lock = ProjectLock::try_acquire(&first, &temp.lock_root())?;

    let error = LockedProject::bind(&lock, &second)
        .err()
        .ok_or("mismatched lock unexpectedly bound")?;
    let message = error.to_string();

    assert!(matches!(
        error,
        TransactionModelError::LockProjectMismatch { .. }
    ));
    assert!(!message.contains("SECRET_FIRST_PROJECT"));
    assert!(!message.contains("SECRET_SECOND_PROJECT"));
    Ok(())
}

#[test]
fn relative_dot_and_parent_project_aliases_bind_to_lock() -> Result<(), Box<dyn std::error::Error>>
{
    let temp = TestDirectory::new()?;
    let project = temp.project("project")?;
    fs::create_dir(project.join("child"))?;
    let lock = ProjectLock::try_acquire(&project, &temp.lock_root())?;
    let relative = relative_path(&std::env::current_dir()?, &project)
        .ok_or("test paths use different roots")?;

    LockedProject::bind(&lock, &relative.join("."))?;
    LockedProject::bind(&lock, &project.join("child").join(".."))?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn symbolic_link_project_alias_binds_to_lock() -> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;
    let temp = TestDirectory::new()?;
    let project = temp.project("project")?;
    let alias = temp.0.join("alias");
    symlink(&project, &alias)?;
    let lock = ProjectLock::try_acquire(&project, &temp.lock_root())?;

    LockedProject::bind(&lock, &alias)?;
    Ok(())
}

#[test]
fn accepts_normal_and_korean_target_paths_with_forward_slashes(
) -> Result<(), Box<dyn std::error::Error>> {
    let normal = ProjectRelativePath::parse(r"data\characters\hero.json")?;
    let korean = ProjectRelativePath::parse("자료/세계/설정.json")?;

    assert_eq!(normal.as_str(), "data/characters/hero.json");
    assert_eq!(korean.as_str(), "자료/세계/설정.json");
    assert_eq!(korean.to_string(), "자료/세계/설정.json");
    Ok(())
}

#[test]
fn rejects_absolute_windows_prefixed_empty_and_filename_less_targets() {
    for value in [
        "/data/file.json",
        r"C:\data\file.json",
        r"\\server\share\file.json",
        "",
        "folder/",
    ] {
        assert!(matches!(
            ProjectRelativePath::parse(value).map_err(TransactionModelError::from),
            Err(TransactionModelError::InvalidTargetPath { .. })
        ));
    }
}

#[test]
fn rejects_dot_parent_and_reserved_system_targets() {
    for value in [
        ".",
        "data/./file.json",
        "..",
        "data/../file.json",
        ".worldbuild",
        ".worldbuild/transactions/file.json",
        ".WORLDBUILD/state.json",
    ] {
        assert!(ProjectRelativePath::parse(value).is_err(), "{value}");
    }
}

#[test]
fn separator_aliases_are_equal_and_windows_ascii_case_is_documented(
) -> Result<(), Box<dyn std::error::Error>> {
    let slash = ProjectRelativePath::parse("data/file.json")?;
    let backslash = ProjectRelativePath::parse(r"data\file.json")?;
    assert_eq!(slash, backslash);

    #[cfg(windows)]
    assert_eq!(
        ProjectRelativePath::parse("DATA/File.JSON")?,
        ProjectRelativePath::parse("data/file.json")?
    );
    #[cfg(not(windows))]
    assert_ne!(
        ProjectRelativePath::parse("DATA/File.JSON")?,
        ProjectRelativePath::parse("data/file.json")?
    );
    Ok(())
}

#[test]
fn transaction_ids_are_fixed_safe_and_unique_candidates() -> Result<(), Box<dyn std::error::Error>>
{
    let (first, first_time) = TransactionId::new_candidate(HASH_A)?;
    let (second, second_time) = TransactionId::new_candidate(HASH_A)?;

    assert_ne!(first, second);
    assert_eq!(first.as_str().len(), 68);
    assert!(first.as_str().starts_with("txn-"));
    assert!(first
        .as_str()
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'));
    assert!(super::super::utc_time::is_utc_milliseconds(&first_time));
    assert!(super::super::utc_time::is_utc_milliseconds(&second_time));
    assert_eq!(TransactionId::parse(first.as_str())?, first);
    Ok(())
}

#[test]
fn rejects_transaction_id_path_manipulation_and_bad_hex() {
    for value in [
        "../txn-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "txn-abc",
        "txn-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "other-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert!(matches!(
            TransactionId::parse(value),
            Err(TransactionModelError::InvalidTransactionId { .. })
        ));
    }
}

#[test]
fn valid_manifest_round_trips_as_deterministic_json() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = valid_manifest()?;
    manifest.validate()?;
    let bytes = to_deterministic_json_bytes(&manifest)?;
    let decoded: TransactionManifest = serde_json::from_slice(&bytes)?;
    let text = String::from_utf8(bytes.clone())?;

    decoded.validate()?;
    assert_eq!(decoded, manifest);
    assert_eq!(to_deterministic_json_bytes(&decoded)?, bytes);
    assert_eq!(TRANSACTION_SCHEMA_VERSION.get(), 1);
    assert_eq!(serde_json::to_value(&decoded)?["schemaVersion"], 1);
    // 공통 값 타입으로 이동해도 기존 manifest의 필드명과 상대 경로 바이트는 그대로다.
    assert!(text.contains("\"targetPath\": \"data/a.json\""));
    assert!(text.contains("\"targetPath\": \"data/b.json\""));
    assert!(!text.contains("project body secret"));
    Ok(())
}

#[test]
fn manifest_deserialization_revalidates_target_path() -> Result<(), Box<dyn std::error::Error>> {
    let manifest = valid_manifest()?;
    let mut value = serde_json::to_value(manifest)?;
    value["operations"][0]["targetPath"] = serde_json::Value::String("../outside.json".into());

    assert!(serde_json::from_value::<TransactionManifest>(value).is_err());
    Ok(())
}

#[test]
fn rejects_empty_operations_and_discontinuous_indexes() -> Result<(), Box<dyn std::error::Error>> {
    let mut empty = valid_manifest()?;
    empty.operations.clear();
    assert!(matches!(
        empty.validate(),
        Err(TransactionModelError::InvalidJournal { .. })
    ));

    let mut discontinuous = valid_manifest()?;
    discontinuous.operations[1].index = 3;
    assert!(matches!(
        discontinuous.validate(),
        Err(TransactionModelError::InvalidJournal { .. })
    ));
    Ok(())
}

#[test]
fn rejects_duplicate_and_unsorted_targets() -> Result<(), Box<dyn std::error::Error>> {
    let mut duplicate = valid_manifest()?;
    duplicate.operations[1].target_path = duplicate.operations[0].target_path.clone();
    duplicate.operations[1].staged_path = staged_artifact_path(1);
    duplicate.operations[1].backup_path = backup_artifact_path(1).into();
    assert!(matches!(
        duplicate.validate(),
        Err(TransactionModelError::DuplicateTarget { .. })
    ));

    let mut unsorted = valid_manifest()?;
    unsorted.operations.swap(0, 1);
    unsorted.operations[0].index = 0;
    unsorted.operations[0].staged_path = staged_artifact_path(0);
    unsorted.operations[0].backup_path = backup_artifact_path(0).into();
    unsorted.operations[1].index = 1;
    unsorted.operations[1].staged_path = staged_artifact_path(1);
    unsorted.operations[1].backup_path = backup_artifact_path(1).into();
    assert!(matches!(
        unsorted.validate(),
        Err(TransactionModelError::InvalidJournal { .. })
    ));
    Ok(())
}

#[test]
fn rejects_fixed_artifact_path_and_sha256_violations() -> Result<(), Box<dyn std::error::Error>> {
    let mut staged_path = valid_manifest()?;
    staged_path.operations[0].staged_path = "../outside.json".to_owned();
    assert!(staged_path.validate().is_err());

    let mut staged_hash = valid_manifest()?;
    staged_hash.operations[0].staged_sha256 = "ABC".to_owned();
    assert!(staged_hash.validate().is_err());

    let mut original_hash = valid_manifest()?;
    original_hash.operations[0].original_sha256 = Some("0".repeat(63));
    assert!(original_hash.validate().is_err());
    Ok(())
}

#[test]
fn enforces_original_existence_backup_size_and_hash_consistency(
) -> Result<(), Box<dyn std::error::Error>> {
    let mut missing_backup = valid_manifest()?;
    missing_backup.operations[0].backup_path = None;
    assert!(missing_backup.validate().is_err());

    let mut new_with_original = valid_manifest()?;
    let operation = &mut new_with_original.operations[1];
    operation.original_existed = false;
    operation.backup_path = Some(backup_artifact_path(1));
    operation.original_size = Some(1);
    operation.original_sha256 = Some(HASH_A.to_owned());
    assert!(new_with_original.validate().is_err());

    let mut valid_new = valid_manifest()?;
    let operation = &mut valid_new.operations[1];
    operation.original_existed = false;
    operation.backup_path = None;
    operation.original_size = None;
    operation.original_sha256 = None;
    operation.original_schema_version = None;
    valid_new.validate()?;
    let operation_json = serde_json::to_value(&valid_new.operations[1])?;
    assert!(operation_json.get("backupPath").is_none());
    assert!(operation_json.get("originalSize").is_none());
    assert!(operation_json.get("originalSha256").is_none());
    assert!(operation_json.get("originalSchemaVersion").is_none());
    Ok(())
}

#[test]
fn schema_version_fields_preserve_older_manifest_json_compatibility(
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = valid_manifest()?;
    let mut value = serde_json::to_value(&manifest)?;
    let operations = value
        .get_mut("operations")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or("operations missing")?;
    for operation in operations {
        let object = operation.as_object_mut().ok_or("operation is not object")?;
        object.remove("stagedSchemaVersion");
        object.remove("originalSchemaVersion");
    }
    let decoded: TransactionManifest = serde_json::from_value(value)?;
    decoded.validate()?;
    assert!(decoded.operations.iter().all(|operation| {
        operation.staged_schema_version.is_none() && operation.original_schema_version.is_none()
    }));
    Ok(())
}

#[test]
fn rejects_bad_timestamp_and_project_fingerprint() -> Result<(), Box<dyn std::error::Error>> {
    for timestamp in [
        "2026-08-31T01:23:45Z",
        "2026-08-31T01:23:45.67Z",
        "2026-13-31T01:23:45.678Z",
        "2026-08-31T01:23:45.678+00:00",
    ] {
        let mut manifest = valid_manifest()?;
        manifest.created_at_utc = timestamp.to_owned();
        assert!(manifest.validate().is_err(), "{timestamp}");
    }
    let mut manifest = valid_manifest()?;
    manifest.project_fingerprint = "A".repeat(64);
    assert!(manifest.validate().is_err());
    Ok(())
}

#[test]
fn unknown_transaction_schema_is_reported_separately() -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = valid_manifest()?;
    manifest.schema_version = SchemaVersion::try_from(3)?;

    assert!(matches!(
        manifest.validate(),
        Err(TransactionModelError::UnsupportedTransactionSchema {
            found: 3,
            supported: 2
        })
    ));
    Ok(())
}

#[test]
fn transaction_states_use_camel_case_json() -> Result<(), Box<dyn std::error::Error>> {
    for (state, expected) in [
        (TransactionState::Preparing, "\"preparing\""),
        (TransactionState::Prepared, "\"prepared\""),
        (TransactionState::Applying, "\"applying\""),
        (TransactionState::RollingBack, "\"rollingBack\""),
        (TransactionState::Committed, "\"committed\""),
        (TransactionState::RolledBack, "\"rolledBack\""),
    ] {
        assert_eq!(serde_json::to_string(&state)?, expected);
    }
    Ok(())
}

#[test]
fn state_record_validates_common_fields_and_progress_prefix(
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = valid_manifest()?;
    let mut state = TransactionStateRecord {
        original_targets: None,
        schema_version: TRANSACTION_SCHEMA_VERSION,
        transaction_id: manifest.transaction_id,
        updated_at_utc: VALID_TIME.to_owned(),
        project_fingerprint: HASH_A.to_owned(),
        state: TransactionState::Applying,
        applied_operations: vec![0, 1],
    };
    state.validate()?;
    state.applied_operations = vec![0, 2];
    assert!(state.validate().is_err());
    Ok(())
}

#[test]
fn committed_and_rolled_back_markers_validate_independently(
) -> Result<(), Box<dyn std::error::Error>> {
    let (id, _) = TransactionId::new_candidate(HASH_A)?;
    let committed = CommittedMarker {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        transaction_id: id.clone(),
        completed_at_utc: VALID_TIME.to_owned(),
        project_fingerprint: HASH_A.to_owned(),
    };
    let mut rolled_back = RolledBackMarker {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        transaction_id: id,
        completed_at_utc: VALID_TIME.to_owned(),
        project_fingerprint: HASH_A.to_owned(),
    };
    committed.validate()?;
    rolled_back.validate()?;
    rolled_back.completed_at_utc = "invalid".to_owned();
    assert!(rolled_back.validate().is_err());
    assert_ne!(
        serde_json::to_value(committed)?,
        serde_json::to_value(rolled_back)?
    );
    Ok(())
}

fn valid_manifest() -> Result<TransactionManifest, Box<dyn std::error::Error>> {
    let (transaction_id, _) = TransactionId::new_candidate(HASH_A)?;
    Ok(TransactionManifest {
        schema_version: TRANSACTION_SCHEMA_VERSION,
        transaction_id,
        created_at_utc: VALID_TIME.to_owned(),
        project_fingerprint: HASH_A.to_owned(),
        operations: vec![
            TransactionOperation {
                index: 0,
                target_path: ProjectRelativePath::parse("data/a.json")?,
                staged_path: staged_artifact_path(0),
                backup_path: Some(backup_artifact_path(0)),
                original_existed: true,
                original_size: Some(10),
                original_sha256: Some(HASH_A.to_owned()),
                staged_size: 11,
                staged_sha256: HASH_B.to_owned(),
                staged_schema_version: Some(TRANSACTION_SCHEMA_VERSION),
                original_schema_version: Some(TRANSACTION_SCHEMA_VERSION),
            },
            TransactionOperation {
                index: 1,
                target_path: ProjectRelativePath::parse("data/b.json")?,
                staged_path: staged_artifact_path(1),
                backup_path: Some(backup_artifact_path(1)),
                original_existed: true,
                original_size: Some(20),
                original_sha256: Some(HASH_B.to_owned()),
                staged_size: 21,
                staged_sha256: HASH_A.to_owned(),
                staged_schema_version: Some(TRANSACTION_SCHEMA_VERSION),
                original_schema_version: Some(TRANSACTION_SCHEMA_VERSION),
            },
        ],
    })
}

fn relative_path(from: &Path, to: &Path) -> Option<PathBuf> {
    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();
    let shared = from_components
        .iter()
        .zip(&to_components)
        .take_while(|(left, right)| left == right)
        .count();
    if shared == 0 {
        return None;
    }
    let mut relative = PathBuf::new();
    for _ in &from_components[shared..] {
        relative.push("..");
    }
    for component in &to_components[shared..] {
        relative.push(component.as_os_str());
    }
    Some(relative)
}
