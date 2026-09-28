use super::*;
use crate::data::assets::Metadata;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

struct Fixture(PathBuf);
impl Fixture {
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0.parent() == Some(std::env::temp_dir().as_path()) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn root() -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "worldbuild-asset-maintenance-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir(&path).expect("fixture");
    Fixture(path)
}

fn asset(root: &Path, id: &str, name: &str, bytes: &[u8]) {
    let store = Store::open(root, true).expect("asset store");
    let metadata = Metadata {
        schema_version: 1,
        id: id.into(),
        name: name.into(),
        size: bytes.len() as u64,
        sha256: digest(bytes),
        image: false,
        width: None,
        height: None,
    };
    store.put(&metadata, bytes).expect("asset");
}

fn deleted_template(root: &Path, id: &str, name: &str) {
    fs::create_dir_all(root.join("templates")).unwrap();
    fs::write(
        root.join("templates").join(format!("{id}.json")),
        serde_json::to_vec_pretty(&json!({
            "artifactType": "template",
            "createdAtUtc": "2026-09-21T00:00:00.000Z",
            "fieldOrder": [],
            "fields": {},
            "lifecycle": "deleted",
            "name": name,
            "presentation": {},
            "revision": 1,
            "schemaVersion": 5,
            "templateId": id,
            "updatedAtUtc": "2026-09-21T00:00:00.000Z"
        }))
        .unwrap(),
    )
    .unwrap();
}

fn trashed_document(root: &Path, id: &str) {
    fs::create_dir_all(root.join("documents")).unwrap();
    fs::create_dir_all(root.join("workspace")).unwrap();
    fs::write(
        root.join("documents").join(format!("{id}.json")),
        serde_json::to_vec(&json!({
            "artifactType":"document",
            "schemaVersion":1,
            "documentId":id,
            "templateId":"11111111-1111-4111-8111-111111111111",
            "templateRevision":1,
            "name":"purge target",
            "fieldValues":{},
            "orphanedFieldDefinitions":{},
            "createdAtUtc":"2026-09-21T00:00:00.000Z",
            "updatedAtUtc":"2026-09-21T00:00:00.000Z"
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        root.join("workspace/document-layout.json"),
        serde_json::to_vec(&json!({
            "artifactType":"documentLayout",
            "schemaVersion":1,
            "revision":2,
            "rootOrder":[],
            "nodes":{
                (id):{
                    "parentId":null,
                    "childOrder":[],
                    "state":"trashed",
                    "trash":{
                        "parentId":null,
                        "index":0,
                        "trashedAtUtc":"2026-09-21T00:00:00.000Z"
                    }
                }
            }
        }))
        .unwrap(),
    )
    .unwrap();
}

fn publish_layout_without_document(root: &Path) {
    fs::write(
        root.join("workspace/document-layout.json"),
        serde_json::to_vec(&json!({
            "artifactType":"documentLayout",
            "schemaVersion":1,
            "revision":3,
            "rootOrder":[],
            "nodes":{}
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn document_purge_follows_layout_commit_and_rollback() {
    let committed = root();
    let committed_id = uuid::Uuid::new_v4().to_string();
    trashed_document(committed.path(), &committed_id);
    let purge = begin_document_purge(committed.path(), &committed_id).unwrap();
    publish_layout_without_document(committed.path());
    assert!(!finish_document_purge(committed.path(), purge, true).unwrap());
    assert!(!committed
        .path()
        .join("documents")
        .join(format!("{committed_id}.json"))
        .exists());
    assert!(!has_pending(committed.path()).unwrap());

    let rolled_back = root();
    let rolled_back_id = uuid::Uuid::new_v4().to_string();
    trashed_document(rolled_back.path(), &rolled_back_id);
    let purge = begin_document_purge(rolled_back.path(), &rolled_back_id).unwrap();
    assert!(!finish_document_purge(rolled_back.path(), purge, false).unwrap());
    assert!(rolled_back
        .path()
        .join("documents")
        .join(format!("{rolled_back_id}.json"))
        .exists());
    assert!(!has_pending(rolled_back.path()).unwrap());
}

#[test]
fn document_purge_recovery_finishes_committed_layout_once() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    trashed_document(fixture.path(), &id);
    let purge = begin_document_purge(fixture.path(), &id).unwrap();
    publish_layout_without_document(fixture.path());
    drop(purge);
    assert!(has_pending(fixture.path()).unwrap());
    recover_pending(fixture.path()).unwrap();
    recover_pending(fixture.path()).unwrap();
    assert!(!fixture
        .path()
        .join("documents")
        .join(format!("{id}.json"))
        .exists());
    assert!(!has_pending(fixture.path()).unwrap());
}

fn small_limits() -> InspectionLimits {
    InspectionLimits {
        files: 32,
        entries: 32,
        directories: 32,
        depth: 8,
        handles: 64,
        file_bytes: 4096,
        total_bytes: 64 * 1024,
        references: 32,
        buffer_bytes: 4096,
    }
}

#[test]
fn production_inspection_traversal_enforces_each_injected_cap_at_plus_one() {
    let file_fixture = root();
    fs::create_dir(file_fixture.path().join("documents")).unwrap();
    fs::write(file_fixture.path().join("documents/one.json"), b"{}").unwrap();
    let mut limits = small_limits();
    limits.files = 1;
    limits.entries = 1;
    assert!(with_inspection_limits(limits, || inspect(file_fixture.path())).is_ok());
    fs::write(file_fixture.path().join("documents/two.json"), b"{}").unwrap();
    assert_eq!(
        with_inspection_limits(limits, || inspect(file_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );

    let directory_fixture = root();
    fs::create_dir_all(directory_fixture.path().join("documents/empty")).unwrap();
    let mut limits = small_limits();
    limits.directories = 2;
    limits.depth = 1;
    limits.entries = 1;
    assert!(with_inspection_limits(limits, || inspect(directory_fixture.path())).is_ok());
    fs::create_dir_all(directory_fixture.path().join("documents/second")).unwrap();
    assert_eq!(
        with_inspection_limits(limits, || inspect(directory_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );

    let depth_fixture = root();
    fs::create_dir_all(depth_fixture.path().join("documents/empty/deeper")).unwrap();
    let mut limits = small_limits();
    limits.directories = 3;
    limits.entries = 2;
    limits.depth = 2;
    assert!(with_inspection_limits(limits, || inspect(depth_fixture.path())).is_ok());
    limits.depth = 1;
    assert_eq!(
        with_inspection_limits(limits, || inspect(depth_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );

    let handle_fixture = root();
    fs::create_dir(handle_fixture.path().join("documents")).unwrap();
    fs::write(handle_fixture.path().join("documents/one.json"), b"{}").unwrap();
    let mut limits = small_limits();
    limits.handles = 3;
    assert!(with_inspection_limits(limits, || inspect(handle_fixture.path())).is_ok());
    limits.handles = 2;
    assert_eq!(
        with_inspection_limits(limits, || inspect(handle_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );

    let bytes_fixture = root();
    fs::create_dir(bytes_fixture.path().join("documents")).unwrap();
    fs::write(bytes_fixture.path().join("documents/one.json"), b"{}").unwrap();
    let mut limits = small_limits();
    limits.file_bytes = 2;
    limits.buffer_bytes = 2;
    limits.total_bytes = 2;
    assert!(with_inspection_limits(limits, || inspect(bytes_fixture.path())).is_ok());
    limits.total_bytes = 1;
    assert_eq!(
        with_inspection_limits(limits, || inspect(bytes_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );
    limits.total_bytes = 2;
    limits.buffer_bytes = 1;
    assert_eq!(
        with_inspection_limits(limits, || inspect(bytes_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );
    limits.buffer_bytes = 2;
    limits.file_bytes = 1;
    assert_eq!(
        with_inspection_limits(limits, || inspect(bytes_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );

    let reference_fixture = root();
    fs::create_dir(reference_fixture.path().join("documents")).unwrap();
    let one = uuid::Uuid::new_v4().to_string();
    let two = uuid::Uuid::new_v4().to_string();
    fs::write(
        reference_fixture.path().join("documents/refs.json"),
        serde_json::to_vec(&json!({"value":{"kind":"file","value":[one.clone()]}})).unwrap(),
    )
    .unwrap();
    let mut limits = small_limits();
    limits.references = 1;
    assert!(with_inspection_limits(limits, || inspect(reference_fixture.path())).is_ok());
    fs::write(
        reference_fixture.path().join("documents/refs.json"),
        serde_json::to_vec(&json!({"value":{"kind":"file","value":[one,two]}})).unwrap(),
    )
    .unwrap();
    assert_eq!(
        with_inspection_limits(limits, || inspect(reference_fixture.path()))
            .unwrap_err()
            .category(),
        Category::TooLarge
    );
}

const CRASH_TEST: &str = "data::asset_maintenance::tests::m545_asset_maintenance_crash_child";
const FOLLOW_UP_DOCUMENT: &str = "77777777-7777-4777-8777-777777777777";

fn repository_follow_up(runtime: &mut crate::data::project_runtime::ProjectRuntime, create: bool) {
    use crate::data::{
        collaboration_lock::{LockCoordinator, LockService, LockSessionId, NoLockService},
        repository::{ArtifactRepository, CanonicalWritePlan},
    };
    let ready = runtime
        .ready()
        .expect("reader is issued after maintenance recovery");
    let repository = ArtifactRepository::new(&ready).expect("repository opens after recovery");
    let id = FOLLOW_UP_DOCUMENT.parse().unwrap();
    if create {
        let bytes = serde_json::to_vec(&json!({
            "artifactType":"document",
            "schemaVersion":1,
            "documentId":FOLLOW_UP_DOCUMENT,
            "templateId":"11111111-1111-4111-8111-111111111111",
            "templateRevision":1,
            "name":"asset recovery follow-up",
            "fieldValues":{},
            "orphanedFieldDefinitions":{},
            "createdAtUtc":"2026-09-21T00:00:00.000Z",
            "updatedAtUtc":"2026-09-21T00:00:00.000Z",
            "future":null
        }))
        .unwrap();
        let document = crate::data::artifact::decode_document(&bytes).unwrap();
        let plan = CanonicalWritePlan::new()
            .create_document(&document)
            .unwrap();
        let targets = plan.targets().cloned().collect();
        let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
        let coordinator = LockCoordinator::new(service);
        let mut locks = coordinator
            .acquire_all(
                repository.write_project().fingerprint(),
                &LockSessionId::generate().unwrap(),
                targets,
            )
            .unwrap();
        plan.prepare(&repository, locks.write_permit().unwrap())
            .unwrap()
            .commit()
            .unwrap();
        locks.release_all().unwrap();
    }
    let loaded = repository.load_document(id).unwrap();
    assert_eq!(loaded.artifact().name(), "asset recovery follow-up");
}

#[test]
#[ignore = "controller kills this process after observing the mutation checkpoint"]
fn m545_asset_maintenance_crash_child() {
    let Some(mode) = std::env::var_os("WB_M545_ASSET_CHILD_MODE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("WB_M545_ASSET_ROOT").unwrap());
    let locks = PathBuf::from(std::env::var_os("WB_M545_ASSET_LOCKS").unwrap());
    if mode == "move" {
        let id = std::env::var("WB_M545_ASSET_ID").unwrap();
        let inspection = inspect(&root).unwrap();
        let _ = move_to_trash(&root, &inspection.token, &[id]);
        panic!("move child reached terminal state instead of checkpoint");
    }
    if mode == "restore" {
        let id = std::env::var("WB_M545_ASSET_ID").unwrap();
        let _ = restore_from_trash(&root, &[id]);
        panic!("restore child reached terminal state instead of checkpoint");
    }
    if mode == "purge" {
        let id = std::env::var("WB_M545_ASSET_ID").unwrap();
        let inspection = inspect(&root).unwrap();
        let _ = purge_trash(&root, &inspection.token, &[id], false);
        panic!("purge child reached terminal state instead of checkpoint");
    }
    if mode == "template_purge" {
        let id = std::env::var("WB_M545_ASSET_ID").unwrap();
        let inspection = inspect(&root).unwrap();
        let _ = purge_templates(&root, &inspection.token, &[id]);
        panic!("template purge child reached terminal state instead of checkpoint");
    }
    if mode == "document_purge" {
        let id = std::env::var("WB_M545_ASSET_ID").unwrap();
        let purge = begin_document_purge(&root, &id).unwrap();
        publish_layout_without_document(&root);
        let _ = finish_document_purge(&root, purge, true);
        panic!("document purge child reached terminal state instead of checkpoint");
    }
    if mode == "document_recover" || mode == "document_verify" {
        let id = std::env::var("WB_M545_ASSET_ID").unwrap();
        let canary = PathBuf::from(std::env::var_os("WB_M545_ASSET_CANARY").unwrap());
        let mut runtime = crate::data::project_runtime::ProjectRuntime::acquire(&root, &locks)
            .expect("restart acquires document purge project");
        runtime
            .recover()
            .expect("document purge recovery precedes normal readers");
        assert!(!root.join("documents").join(format!("{id}.json")).exists());
        let layout: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("workspace/document-layout.json")).unwrap())
                .unwrap();
        assert!(layout["nodes"].get(&id).is_none());
        assert_eq!(fs::read(&canary).unwrap(), b"document-purge-canary");
        repository_follow_up(&mut runtime, mode == "document_recover");
        runtime
            .close()
            .expect("document purge restart closes cleanly");
        return;
    }
    let id = std::env::var("WB_M545_ASSET_ID").unwrap();
    let mut runtime = crate::data::project_runtime::ProjectRuntime::acquire(&root, &locks)
        .expect("restart acquires project");
    runtime
        .recover()
        .expect("startup maintenance recovery succeeds");
    let inspection = inspect(&root).unwrap();
    let expected = std::env::var("WB_M545_ASSET_EXPECTED").unwrap_or_default();
    if expected == "absent" {
        assert!(!inspection.rows.iter().any(|row| row.id == id));
        assert!(!inspection.trash.iter().any(|row| row.id == id));
    } else if expected == "active" {
        assert!(inspection.rows.iter().any(|row| row.id == id));
        assert!(!inspection.trash.iter().any(|row| row.id == id));
    } else {
        assert_eq!(
            inspection.trash.iter().filter(|row| row.id == id).count(),
            1
        );
    }
    repository_follow_up(&mut runtime, mode == "recover");
    runtime.close().expect("restart closes cleanly");
}

fn killed_template_purge_case() {
    let fixture = root();
    let lock_fixture = root();
    let locks = lock_fixture.path().to_path_buf();
    let removable = uuid::Uuid::new_v4().to_string();
    let protected = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &removable, "template kill target");
    deleted_template(fixture.path(), &protected, "protected template");
    fs::create_dir_all(fixture.path().join("documents")).unwrap();
    let protected_document = fixture.path().join("documents/protected.json");
    let protected_bytes =
        format!("{{\"templateId\":\"{protected}\",\"canary\":\"kept\"}}").into_bytes();
    fs::write(&protected_document, &protected_bytes).unwrap();
    let ready = fixture.path().join("template-checkpoint-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
        .env("WB_M545_ASSET_CHILD_MODE", "template_purge")
        .env("WB_M545_ASSET_CRASH_POINT", "after_template_delete")
        .env("WB_M545_ASSET_CRASH_READY", &ready)
        .env("WB_M545_ASSET_ROOT", fixture.path())
        .env("WB_M545_ASSET_LOCKS", &locks)
        .env("WB_M545_ASSET_ID", &removable)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("template purge child exited before mutation checkpoint: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "template purge mutation was not observed");
    assert!(!fixture
        .path()
        .join("templates")
        .join(format!("{removable}.json"))
        .exists());
    assert!(fixture
        .path()
        .join("templates")
        .join(format!("{protected}.json"))
        .exists());
    assert_eq!(fs::read(&protected_document).unwrap(), protected_bytes);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(has_pending(fixture.path()).unwrap());
    for mode in ["recover", "verify"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
            .env("WB_M545_ASSET_CHILD_MODE", mode)
            .env("WB_M545_ASSET_ROOT", fixture.path())
            .env("WB_M545_ASSET_LOCKS", &locks)
            .env("WB_M545_ASSET_ID", &removable)
            .env("WB_M545_ASSET_EXPECTED", "absent")
            .status()
            .unwrap();
        assert!(status.success(), "{mode} child failed");
    }
    assert!(!has_pending(fixture.path()).unwrap());
    assert!(fixture
        .path()
        .join("templates")
        .join(format!("{protected}.json"))
        .exists());
    assert_eq!(fs::read(&protected_document).unwrap(), protected_bytes);
}

fn killed_document_purge_case() {
    let fixture = root();
    let lock_fixture = root();
    let locks = lock_fixture.path().to_path_buf();
    let id = uuid::Uuid::new_v4().to_string();
    trashed_document(fixture.path(), &id);
    let canary_id = uuid::Uuid::new_v4().to_string();
    let canary = fixture
        .path()
        .join("documents")
        .join(format!("{canary_id}.json"));
    fs::write(&canary, b"document-purge-canary").unwrap();
    let ready = fixture.path().join("document-purge-checkpoint-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
        .env("WB_M545_ASSET_CHILD_MODE", "document_purge")
        .env("WB_M545_ASSET_CRASH_POINT", "after_document_delete")
        .env("WB_M545_ASSET_CRASH_READY", &ready)
        .env("WB_M545_ASSET_ROOT", fixture.path())
        .env("WB_M545_ASSET_LOCKS", &locks)
        .env("WB_M545_ASSET_ID", &id)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("document purge child exited before mutation checkpoint: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "document purge mutation was not observed");
    assert!(!fixture
        .path()
        .join("documents")
        .join(format!("{id}.json"))
        .exists());
    assert_eq!(fs::read(&canary).unwrap(), b"document-purge-canary");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(has_pending(fixture.path()).unwrap());
    for mode in ["document_recover", "document_verify"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
            .env("WB_M545_ASSET_CHILD_MODE", mode)
            .env("WB_M545_ASSET_ROOT", fixture.path())
            .env("WB_M545_ASSET_LOCKS", &locks)
            .env("WB_M545_ASSET_ID", &id)
            .env("WB_M545_ASSET_CANARY", &canary)
            .status()
            .unwrap();
        assert!(status.success(), "{mode} child failed");
    }
    assert!(!has_pending(fixture.path()).unwrap());
    assert_eq!(fs::read(&canary).unwrap(), b"document-purge-canary");
}

fn killed_purge_case() {
    let fixture = root();
    let lock_fixture = root();
    let locks = lock_fixture.path().to_path_buf();
    fs::create_dir(fixture.path().join("documents")).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "purge-kill.bin", b"purge-boundary");
    let inspection = inspect(fixture.path()).unwrap();
    move_to_trash(fixture.path(), &inspection.token, std::slice::from_ref(&id)).unwrap();
    let ready = fixture.path().join("checkpoint-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
        .env("WB_M545_ASSET_CHILD_MODE", "purge")
        .env("WB_M545_ASSET_CRASH_POINT", "after_purge_file")
        .env("WB_M545_ASSET_CRASH_READY", &ready)
        .env("WB_M545_ASSET_ROOT", fixture.path())
        .env("WB_M545_ASSET_LOCKS", &locks)
        .env("WB_M545_ASSET_ID", &id)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("purge child exited before mutation checkpoint: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(
        ready.exists(),
        "purge child did not reach mutation checkpoint"
    );
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(has_pending(fixture.path()).unwrap());
    let package = fixture.path().join("assets/.trash").join(&id);
    assert!(package.exists());
    assert!(fs::read_dir(&package).unwrap().count() < 3);
    for mode in ["recover", "verify"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
            .env("WB_M545_ASSET_CHILD_MODE", mode)
            .env("WB_M545_ASSET_ROOT", fixture.path())
            .env("WB_M545_ASSET_LOCKS", &locks)
            .env("WB_M545_ASSET_ID", &id)
            .env("WB_M545_ASSET_EXPECTED", "absent")
            .status()
            .unwrap();
        assert!(status.success(), "{mode} child failed");
    }
    assert!(!package.exists());
    assert!(!has_pending(fixture.path()).unwrap());
}

fn killed_move_case(point: &str) {
    let fixture = root();
    let lock_fixture = root();
    let locks = lock_fixture.path().to_path_buf();
    fs::create_dir(fixture.path().join("documents")).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "kill.bin", b"kill-boundary");
    let ready = fixture.path().join("checkpoint-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
        .env("WB_M545_ASSET_CHILD_MODE", "move")
        .env("WB_M545_ASSET_CRASH_POINT", point)
        .env("WB_M545_ASSET_CRASH_READY", &ready)
        .env("WB_M545_ASSET_ROOT", fixture.path())
        .env("WB_M545_ASSET_LOCKS", &locks)
        .env("WB_M545_ASSET_ID", &id)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("asset child exited before {point}: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "child did not reach {point}");
    assert!(!fixture.path().join("assets").join(&id).exists());
    assert!(fixture.path().join("assets/.trash").join(&id).exists());
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(has_pending(fixture.path()).unwrap());

    for mode in ["recover", "verify"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
            .env("WB_M545_ASSET_CHILD_MODE", mode)
            .env("WB_M545_ASSET_ROOT", fixture.path())
            .env("WB_M545_ASSET_LOCKS", &locks)
            .env("WB_M545_ASSET_ID", &id)
            .env("WB_M545_ASSET_EXPECTED", "trash")
            .status()
            .unwrap();
        assert!(status.success(), "{mode} child failed");
    }
    assert!(!has_pending(fixture.path()).unwrap());
}

fn killed_restore_case(point: &str) {
    let fixture = root();
    let lock_fixture = root();
    let locks = lock_fixture.path().to_path_buf();
    fs::create_dir(fixture.path().join("documents")).unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "restore-kill.bin", b"restore-boundary");
    let inspection = inspect(fixture.path()).unwrap();
    move_to_trash(fixture.path(), &inspection.token, std::slice::from_ref(&id)).unwrap();
    let ready = fixture.path().join("checkpoint-ready");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
        .env("WB_M545_ASSET_CHILD_MODE", "restore")
        .env("WB_M545_ASSET_CRASH_POINT", point)
        .env("WB_M545_ASSET_CRASH_READY", &ready)
        .env("WB_M545_ASSET_ROOT", fixture.path())
        .env("WB_M545_ASSET_LOCKS", &locks)
        .env("WB_M545_ASSET_ID", &id)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !ready.exists() && Instant::now() < deadline {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("restore child exited before {point}: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(ready.exists(), "child did not reach {point}");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(has_pending(fixture.path()).unwrap());
    for mode in ["recover", "verify"] {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CRASH_TEST, "--ignored", "--nocapture"])
            .env("WB_M545_ASSET_CHILD_MODE", mode)
            .env("WB_M545_ASSET_ROOT", fixture.path())
            .env("WB_M545_ASSET_LOCKS", &locks)
            .env("WB_M545_ASSET_ID", &id)
            .env("WB_M545_ASSET_EXPECTED", "active")
            .status()
            .unwrap();
        assert!(status.success(), "{mode} child failed");
    }
    assert!(!has_pending(fixture.path()).unwrap());
}

#[test]
fn killed_child_recovers_after_asset_rename_and_manifest_publish() {
    killed_move_case("after_asset_rename");
    killed_move_case("after_trash_manifest");
}

#[test]
fn killed_child_recovers_after_restore_manifest_removal_and_asset_rename() {
    killed_restore_case("after_restore_manifest_removed");
    killed_restore_case("after_asset_restore_rename");
}

#[test]
fn killed_child_finishes_confirmed_purge_without_touching_other_project_data() {
    killed_purge_case();
}

#[test]
fn killed_child_finishes_confirmed_template_purge_and_two_restarts() {
    killed_template_purge_case();
}

#[test]
fn killed_child_finishes_document_purge_before_readers_and_two_restarts() {
    killed_document_purge_case();
}

#[test]
fn inspection_distinguishes_used_unused_and_missing_assets() {
    let fixture = root();
    fs::create_dir(fixture.path().join("documents")).unwrap();
    let used = uuid::Uuid::new_v4().to_string();
    let unused = uuid::Uuid::new_v4().to_string();
    let missing = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &used, "used.bin", b"used");
    asset(fixture.path(), &unused, "unused.bin", b"unused");
    fs::write(
        fixture.path().join("documents/d.json"),
        format!("{{\"values\":[{{\"kind\":\"file\",\"value\":[\"{used}\",\"{missing}\"]}}]}}"),
    )
    .unwrap();

    let result = inspect(fixture.path()).unwrap();
    assert!(result.complete);
    assert_eq!(result.used_assets, 1);
    assert_eq!(result.unused_assets, 1);
    assert_eq!(result.missing_assets, 1);
}

#[test]
fn inspection_reports_document_specific_resource_and_deleted_template_problems() {
    let fixture = root();
    fs::create_dir(fixture.path().join("documents")).unwrap();
    let document = uuid::Uuid::new_v4().to_string();
    let missing = uuid::Uuid::new_v4().to_string();
    let template = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &template, "deleted owner");
    fs::write(
        fixture.path().join("documents/document.json"),
        serde_json::to_vec(&json!({
            "artifactType":"document",
            "documentId":document,
            "templateId":template,
            "values":[{"kind":"file","value":[missing]}]
        }))
        .unwrap(),
    )
    .unwrap();

    let result = inspect(fixture.path()).unwrap();
    assert_eq!(result.document_issues.len(), 1);
    assert_eq!(result.document_issues[0].document_id, document);
    assert_eq!(
        result.document_issues[0].reasons,
        ["deleted_template", "resource_missing"]
    );
    assert_eq!(result.document_issues[0].related_resource_ids, [missing]);
    assert_eq!(result.document_issues[0].related_template_ids, [template]);
}

#[test]
fn referenced_asset_in_trash_keeps_identity_name_size_and_document_relation() {
    let fixture = root();
    let document = uuid::Uuid::new_v4().to_string();
    let asset_id = uuid::Uuid::new_v4().to_string();
    let bytes = b"referenced attachment";
    asset(fixture.path(), &asset_id, "attachment.txt", bytes);
    let before = inspect(fixture.path()).unwrap();
    let moved = move_to_trash(
        fixture.path(),
        &before.token,
        std::slice::from_ref(&asset_id),
    )
    .unwrap();
    assert_eq!(moved.completed, [asset_id.clone()]);

    fs::create_dir_all(fixture.path().join("documents")).unwrap();
    fs::write(
        fixture.path().join("documents/document.json"),
        serde_json::to_vec(&json!({
            "artifactType":"document",
            "documentId":document,
            "values":[{"kind":"file","value":[asset_id]}]
        }))
        .unwrap(),
    )
    .unwrap();

    let result = inspect(fixture.path()).unwrap();
    let row = result.rows.iter().find(|row| row.id == asset_id).unwrap();
    assert_eq!(row.status, AssetStatus::InTrash);
    assert_eq!(row.name.as_deref(), Some("attachment.txt"));
    assert_eq!(row.size, Some(bytes.len() as u64));
    assert_eq!(result.document_issues.len(), 1);
    assert_eq!(result.document_issues[0].document_id, document);
    assert_eq!(result.document_issues[0].reasons, ["resource_in_trash"]);
    assert_eq!(result.document_issues[0].related_resource_ids, [asset_id]);
}

#[test]
fn mixed_resource_reasons_keep_their_own_document_targets() {
    let fixture = root();
    let document = uuid::Uuid::new_v4().to_string();
    let trashed = uuid::Uuid::new_v4().to_string();
    let corrupt = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &trashed, "same.txt", b"trash");
    asset(fixture.path(), &corrupt, "same.txt", b"valid");
    let before = inspect(fixture.path()).unwrap();
    assert_eq!(
        move_to_trash(
            fixture.path(),
            &before.token,
            std::slice::from_ref(&trashed)
        )
        .unwrap()
        .completed,
        [trashed.clone()]
    );
    fs::write(
        fixture
            .path()
            .join("assets")
            .join(&corrupt)
            .join("content.txt"),
        b"changed",
    )
    .unwrap();
    fs::create_dir_all(fixture.path().join("documents")).unwrap();
    fs::write(
        fixture.path().join("documents/document.json"),
        serde_json::to_vec(&json!({
            "artifactType":"document",
            "documentId":document,
            "values":[{"kind":"file","value":[trashed, corrupt]}]
        }))
        .unwrap(),
    )
    .unwrap();

    let inspected = inspect(fixture.path()).unwrap();
    let issue = &inspected.document_issues[0];
    assert_eq!(issue.reasons, ["resource_corrupt", "resource_in_trash"]);
    let by_reason = issue
        .targets
        .iter()
        .map(|target| (target.reason, target.resource_ids.clone()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(by_reason["resource_in_trash"], [trashed]);
    assert_eq!(by_reason["resource_corrupt"], [corrupt]);
}

#[test]
fn unused_asset_round_trips_through_trash_with_original_identity_and_bytes() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "roundtrip.bin", b"roundtrip");
    let first = inspect(fixture.path()).unwrap();
    let moved = move_to_trash(fixture.path(), &first.token, std::slice::from_ref(&id)).unwrap();
    assert_eq!(moved.completed, [id.clone()], "{:?}", moved.failures);
    assert!(!fixture.path().join("assets").join(&id).exists());
    assert!(fixture.path().join("assets/.trash").join(&id).exists());

    let restored = restore_from_trash(fixture.path(), std::slice::from_ref(&id)).unwrap();
    assert_eq!(restored.completed, [id.clone()]);
    let (metadata, bytes) = Store::open(fixture.path(), false)
        .unwrap()
        .read(&id)
        .unwrap();
    assert_eq!(metadata.id, id);
    assert_eq!(bytes, b"roundtrip");
}

#[test]
fn asset_rename_preserves_identity_bytes_shared_references_and_extension() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    let bytes = b"shared attachment bytes";
    asset(fixture.path(), &id, "before.txt", bytes);
    fs::create_dir_all(fixture.path().join("documents")).unwrap();
    for name in ["one.json", "two.json"] {
        fs::write(
            fixture.path().join("documents").join(name),
            serde_json::to_vec(&json!({"value":{"kind":"file","value":[id.clone()]}})).unwrap(),
        )
        .unwrap();
    }
    let before = inspect(fixture.path()).unwrap();
    assert_eq!(before.used_assets, 1);
    let renamed = rename_asset(fixture.path(), &before.token, &id, "긴 한글 새 이름.txt")
        .expect("same-extension metadata replacement");
    assert_eq!(renamed.completed, [id.clone()]);
    assert_eq!(renamed.inspection.used_assets, 1);
    let (metadata, actual) = Store::open(fixture.path(), false)
        .unwrap()
        .read(&id)
        .unwrap();
    assert_eq!(metadata.id, id);
    assert_eq!(metadata.name, "긴 한글 새 이름.txt");
    assert_eq!(actual, bytes);
    for name in ["one.json", "two.json"] {
        assert!(
            fs::read_to_string(fixture.path().join("documents").join(name))
                .unwrap()
                .contains(&id)
        );
    }
    assert_eq!(
        rename_asset(
            fixture.path(),
            &renamed.inspection.token,
            &id,
            "extension-change.pdf",
        )
        .unwrap_err()
        .category(),
        Category::InvalidInput
    );
}

#[test]
fn stale_scan_token_cannot_move_an_asset() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "first.bin", b"first");
    let first = inspect(fixture.path()).unwrap();
    asset(
        fixture.path(),
        &uuid::Uuid::new_v4().to_string(),
        "second.bin",
        b"second",
    );
    assert_eq!(
        move_to_trash(fixture.path(), &first.token, std::slice::from_ref(&id))
            .unwrap_err()
            .category(),
        Category::Stale
    );
    assert!(fixture.path().join("assets").join(id).exists());
}

#[test]
fn project_copy_preserves_trash_as_restorable_project_data() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "preserved.bin", b"preserved");
    let first = inspect(fixture.path()).unwrap();
    move_to_trash(fixture.path(), &first.token, std::slice::from_ref(&id)).unwrap();

    let destination = root();
    let copy =
        crate::data::project_backup::export_project(fixture.path(), destination.path(), "copy")
            .unwrap();
    let copied = inspect(&copy.root).unwrap();
    assert_eq!(copied.trash.len(), 1);
    assert_eq!(copied.trash[0].id, id);
    restore_from_trash(&copy.root, std::slice::from_ref(&id)).unwrap();
    let (_, bytes) = Store::open(&copy.root, false).unwrap().read(&id).unwrap();
    assert_eq!(bytes, b"preserved");
}

#[test]
fn manual_backup_and_new_restore_preserve_trash_as_restorable_project_data() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(
        fixture.path(),
        &id,
        "backup-preserved.bin",
        b"backup-preserved",
    );
    let first = inspect(fixture.path()).unwrap();
    move_to_trash(fixture.path(), &first.token, std::slice::from_ref(&id)).unwrap();

    let storage = root();
    let fingerprint = "a".repeat(64);
    let backup = crate::data::project_backup::create_backup(
        fixture.path(),
        &fingerprint,
        storage.path(),
        None,
        crate::data::project_backup::BackupKind::Manual,
    )
    .unwrap();
    let destination = root();
    let restored = crate::data::project_backup::restore_new(
        Path::new(&backup.locator),
        destination.path(),
        "restored",
    )
    .unwrap();
    let inspection = inspect(&restored.root).unwrap();
    assert_eq!(inspection.trash.len(), 1);
    assert_eq!(inspection.trash[0].id, id);
    restore_from_trash(&restored.root, std::slice::from_ref(&id)).unwrap();
    let (_, bytes) = Store::open(&restored.root, false)
        .unwrap()
        .read(&id)
        .unwrap();
    assert_eq!(bytes, b"backup-preserved");
}

#[test]
fn selected_purge_and_empty_trash_delete_only_owned_unreferenced_packages() {
    let fixture = root();
    let first_id = uuid::Uuid::new_v4().to_string();
    let second_id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &first_id, "first.bin", b"first");
    asset(fixture.path(), &second_id, "second.bin", b"second");
    let initial = inspect(fixture.path()).unwrap();
    move_to_trash(
        fixture.path(),
        &initial.token,
        &[first_id.clone(), second_id.clone()],
    )
    .unwrap();

    let after_move = inspect(fixture.path()).unwrap();
    let selected = purge_trash(
        fixture.path(),
        &after_move.token,
        std::slice::from_ref(&first_id),
        false,
    )
    .unwrap();
    assert_eq!(selected.completed_count, 1);
    assert!(!fixture
        .path()
        .join("assets/.trash")
        .join(&first_id)
        .exists());
    assert!(fixture
        .path()
        .join("assets/.trash")
        .join(&second_id)
        .exists());

    let emptied = purge_trash(fixture.path(), &selected.inspection.token, &[], true).unwrap();
    assert_eq!(emptied.completed_count, 1);
    assert!(emptied.inspection.trash.is_empty());
}

#[test]
fn stored_reference_protects_a_trashed_asset_from_selected_and_empty_purge() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "protected.bin", b"protected");
    let initial = inspect(fixture.path()).unwrap();
    move_to_trash(fixture.path(), &initial.token, std::slice::from_ref(&id)).unwrap();
    fs::create_dir_all(fixture.path().join("workspace")).unwrap();
    fs::write(
        fixture.path().join("workspace/state.json"),
        format!("{{\"unknownFutureValue\":{{\"kind\":\"file\",\"value\":[\"{id}\"]}}}}"),
    )
    .unwrap();

    let protected = inspect(fixture.path()).unwrap();
    let row = protected.trash.iter().find(|row| row.id == id).unwrap();
    assert!(row.protected);
    assert_eq!(row.reason, Some("stored_reference"));
    assert_eq!(
        purge_trash(
            fixture.path(),
            &protected.token,
            std::slice::from_ref(&id),
            false,
        )
        .unwrap_err()
        .category(),
        Category::Conflict
    );
    let empty = purge_trash(fixture.path(), &protected.token, &[], true).unwrap();
    assert_eq!(empty.completed_count, 0);
    assert!(fixture.path().join("assets/.trash").join(id).exists());
}

#[test]
fn trash_metadata_change_with_same_content_and_size_invalidates_confirmation_token() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "before.bin", b"same-bytes");
    let initial = inspect(fixture.path()).unwrap();
    move_to_trash(fixture.path(), &initial.token, std::slice::from_ref(&id)).unwrap();
    let confirmed = inspect(fixture.path()).unwrap();
    let metadata_path = fixture
        .path()
        .join("assets/.trash")
        .join(&id)
        .join("metadata.json");
    let mut metadata: serde_json::Value =
        serde_json::from_slice(&fs::read(&metadata_path).unwrap()).unwrap();
    metadata["name"] = json!("after.bin");
    fs::write(&metadata_path, serde_json::to_vec(&metadata).unwrap()).unwrap();

    assert_eq!(
        purge_trash(
            fixture.path(),
            &confirmed.token,
            std::slice::from_ref(&id),
            false,
        )
        .unwrap_err()
        .category(),
        Category::Stale
    );
    assert!(fixture.path().join("assets/.trash").join(id).exists());
}

#[test]
fn completed_mutations_remain_completed_when_intent_cleanup_needs_recovery() {
    let document_fixture = root();
    let document_id = uuid::Uuid::new_v4().to_string();
    trashed_document(document_fixture.path(), &document_id);
    let document_purge = begin_document_purge(document_fixture.path(), &document_id).unwrap();
    publish_layout_without_document(document_fixture.path());
    fail_next_finish(IntentAction::DocumentPurge);
    assert!(finish_document_purge(document_fixture.path(), document_purge, true).unwrap());
    assert!(!document_fixture
        .path()
        .join("documents")
        .join(format!("{document_id}.json"))
        .exists());
    assert!(has_pending(document_fixture.path()).unwrap());
    recover_pending(document_fixture.path()).unwrap();
    assert!(!has_pending(document_fixture.path()).unwrap());

    let move_fixture = root();
    let move_id = uuid::Uuid::new_v4().to_string();
    asset(move_fixture.path(), &move_id, "move.bin", b"move");
    let inspection = inspect(move_fixture.path()).unwrap();
    fail_next_finish(IntentAction::TrashMove);
    let moved = move_to_trash(
        move_fixture.path(),
        &inspection.token,
        std::slice::from_ref(&move_id),
    )
    .unwrap();
    assert_eq!(moved.completed_count, 1);
    assert_eq!(moved.cleanup_required, [move_id.clone()]);
    assert!(move_fixture
        .path()
        .join("assets/.trash")
        .join(&move_id)
        .exists());
    recover_pending(move_fixture.path()).unwrap();

    fail_next_finish(IntentAction::TrashRestore);
    let restored = restore_from_trash(move_fixture.path(), std::slice::from_ref(&move_id)).unwrap();
    assert_eq!(restored.completed_count, 1);
    assert_eq!(restored.cleanup_required, [move_id.clone()]);
    assert!(move_fixture.path().join("assets").join(&move_id).exists());
    recover_pending(move_fixture.path()).unwrap();

    let purge_fixture = root();
    let purge_id = uuid::Uuid::new_v4().to_string();
    asset(purge_fixture.path(), &purge_id, "purge.bin", b"purge");
    let inspection = inspect(purge_fixture.path()).unwrap();
    let moved = move_to_trash(
        purge_fixture.path(),
        &inspection.token,
        std::slice::from_ref(&purge_id),
    )
    .unwrap();
    fail_next_finish(IntentAction::TrashPurge);
    let purged = purge_trash(
        purge_fixture.path(),
        &moved.inspection.token,
        std::slice::from_ref(&purge_id),
        false,
    )
    .unwrap();
    assert_eq!(purged.completed_count, 1);
    assert_eq!(purged.cleanup_required, [purge_id.clone()]);
    assert!(!purge_fixture
        .path()
        .join("assets/.trash")
        .join(&purge_id)
        .exists());
    recover_pending(purge_fixture.path()).unwrap();

    let template_fixture = root();
    let template_id = uuid::Uuid::new_v4().to_string();
    deleted_template(template_fixture.path(), &template_id, "cleanup template");
    let inspection = inspect(template_fixture.path()).unwrap();
    fail_next_finish(IntentAction::TemplatePurge);
    let purged = purge_templates(
        template_fixture.path(),
        &inspection.token,
        std::slice::from_ref(&template_id),
    )
    .unwrap();
    assert_eq!(purged.completed_count, 1);
    assert_eq!(purged.cleanup_required, [template_id.clone()]);
    assert!(!template_fixture
        .path()
        .join("templates")
        .join(format!("{template_id}.json"))
        .exists());
    recover_pending(template_fixture.path()).unwrap();
}

#[test]
fn persisted_intent_rolls_forward_before_normal_readers() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "recover.bin", b"recover");
    let store = Store::open(fixture.path(), false).unwrap();
    let (metadata, _) = store.read(&id).unwrap();
    let _intent = begin_intent(
        fixture.path(),
        IntentAction::TrashMove,
        &id,
        Some(metadata),
        None,
    )
    .unwrap();
    drop(store);
    assert!(has_pending(fixture.path()).unwrap());

    recover_pending(fixture.path()).unwrap();
    assert!(!has_pending(fixture.path()).unwrap());
    assert!(!fixture.path().join("assets").join(&id).exists());
    assert!(fixture.path().join("assets/.trash").join(&id).exists());
    let inspection = inspect(fixture.path()).unwrap();
    assert_eq!(inspection.trash[0].id, id);
}

#[test]
fn deleted_template_purge_requires_complete_unreferenced_evidence_and_fresh_token() {
    let fixture = root();
    let removable = uuid::Uuid::new_v4().to_string();
    let protected = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &removable, "removable");
    deleted_template(fixture.path(), &protected, "protected");
    fs::create_dir_all(fixture.path().join("documents")).unwrap();
    fs::write(
        fixture.path().join("documents/reference.json"),
        format!("{{\"templateId\":\"{protected}\"}}"),
    )
    .unwrap();

    let inspection = inspect(fixture.path()).unwrap();
    assert_eq!(inspection.deleted_templates.len(), 2);
    assert!(
        inspection
            .deleted_templates
            .iter()
            .find(|row| row.id == removable)
            .unwrap()
            .removable
    );
    assert!(
        !inspection
            .deleted_templates
            .iter()
            .find(|row| row.id == protected)
            .unwrap()
            .removable
    );
    assert_eq!(
        purge_templates(
            fixture.path(),
            &inspection.token,
            std::slice::from_ref(&protected),
        )
        .unwrap_err()
        .category(),
        Category::Conflict
    );

    let purged = purge_templates(
        fixture.path(),
        &inspection.token,
        std::slice::from_ref(&removable),
    )
    .unwrap();
    assert_eq!(purged.completed, [removable.clone()]);
    assert!(!fixture
        .path()
        .join("templates")
        .join(format!("{removable}.json"))
        .exists());
    assert!(fixture
        .path()
        .join("templates")
        .join(format!("{protected}.json"))
        .exists());
    assert_eq!(
        purge_templates(
            fixture.path(),
            &inspection.token,
            std::slice::from_ref(&protected),
        )
        .unwrap_err()
        .category(),
        Category::Stale
    );
}

#[test]
fn format_history_unknown_member_protects_deleted_template_definition() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &id, "history protected");
    let history = fixture.path().join(".worldbuild/format-history/document-x");
    fs::create_dir_all(&history).unwrap();
    fs::write(
        history.join("snapshot.json"),
        format!("{{\"futureArchivedEnvelope\":{{\"templateId\":\"{id}\"}}}}"),
    )
    .unwrap();
    let inspection = inspect(fixture.path()).unwrap();
    let row = inspection
        .deleted_templates
        .iter()
        .find(|row| row.id == id)
        .unwrap();
    assert!(!row.removable);
    assert_eq!(row.reason, Some("stored_reference"));
}

#[test]
fn pending_template_purge_is_idempotently_finished_on_restart() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &id, "restart deleted template");
    let path = fixture.path().join("templates").join(format!("{id}.json"));
    let bytes = fs::read(&path).unwrap();
    let intent = begin_intent(
        fixture.path(),
        IntentAction::TemplatePurge,
        &id,
        None,
        Some(digest(&bytes)),
    )
    .unwrap();
    delete_template_file(fixture.path(), &intent).unwrap();
    assert!(!path.exists());
    assert!(has_pending(fixture.path()).unwrap());
    recover_pending(fixture.path()).unwrap();
    assert!(!has_pending(fixture.path()).unwrap());
    assert!(!path.exists());
}

#[cfg(windows)]
#[test]
fn read_only_template_purge_intent_preserves_source_and_unblocks_open() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &id, "read only deleted template");
    let path = fixture.path().join("templates").join(format!("{id}.json"));
    let bytes = fs::read(&path).unwrap();
    let intent = begin_intent(
        fixture.path(),
        IntentAction::TemplatePurge,
        &id,
        None,
        Some(digest(&bytes)),
    )
    .unwrap();
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&path, permissions).unwrap();

    recover_pending(fixture.path()).unwrap();
    assert!(!has_pending(fixture.path()).unwrap());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert!(intent.source_sha256.is_some());
    let mut permissions = fs::metadata(&path).unwrap().permissions();
    permissions.set_readonly(false);
    fs::set_permissions(&path, permissions).unwrap();
}

#[test]
fn changed_template_purge_intent_remains_pending_for_manual_recovery() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    deleted_template(fixture.path(), &id, "deleted template");
    let path = fixture.path().join("templates").join(format!("{id}.json"));
    let original = fs::read(&path).unwrap();
    let _intent = begin_intent(
        fixture.path(),
        IntentAction::TemplatePurge,
        &id,
        None,
        Some(digest(&original)),
    )
    .unwrap();
    fs::write(&path, b"changed after intent").unwrap();

    assert!(recover_pending(fixture.path()).is_err());
    assert!(has_pending(fixture.path()).unwrap());
    assert_eq!(fs::read(&path).unwrap(), b"changed after intent");
}

#[test]
fn snapshot_refuses_a_pending_maintenance_intent() {
    let fixture = root();
    let id = uuid::Uuid::new_v4().to_string();
    asset(fixture.path(), &id, "pending.bin", b"pending");
    let store = Store::open(fixture.path(), false).unwrap();
    let (metadata, _) = store.read(&id).unwrap();
    let _intent = begin_intent(
        fixture.path(),
        IntentAction::TrashMove,
        &id,
        Some(metadata),
        None,
    )
    .unwrap();
    drop(store);
    let output = root();
    let error = crate::data::project_backup::export_project(fixture.path(), output.path(), "copy")
        .unwrap_err();
    assert_eq!(
        error.category(),
        crate::data::project_backup::BackupCategory::SourceChanged
    );
    assert!(!output.path().join("copy").exists());
}

#[test]
#[ignore = "explicit owned M5-4-5 FIX-001 native GUI fixture after U1"]
fn m545_fix001_gui_fixture_reflects_user_purge_results() {
    let fixture = fs::canonicalize(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("logs/M5-4-5-FIX-001/gui/project"),
    )
    .expect("canonical FIX-001 GUI fixture");
    let inspection = inspect(&fixture).expect("post-U1 FIX-001 GUI fixture must inspect");
    assert!(inspection.complete);
    assert!(!inspection.trash.is_empty());
    assert!(inspection.trash.iter().all(|row| row.protected));
    assert!(!inspection.deleted_templates.is_empty());
    assert!(inspection
        .deleted_templates
        .iter()
        .all(|row| !row.removable));
    assert!(!fixture
        .join("assets/.trash/64646464-6464-4646-8646-646464646464")
        .exists());
    assert!(!fixture
        .join("assets/54545454-5454-4545-8545-545454545454")
        .exists());
    assert!(!fixture
        .join("templates/939823d2-0ceb-45df-9c5c-d46f9a2c2277.json")
        .exists());
}
