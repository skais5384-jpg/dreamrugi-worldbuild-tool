use super::*;
mod assets;
mod center;
use crate::data::edit_input::ValueDto;
use model::*;
use std::process::{Command, Stdio};

pub(crate) struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("worldbuild-recovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn root(&self) -> PathBuf {
        self.0.join("edit-recovery")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn sample() -> Envelope {
    Envelope {
        key: Key {
            project_fingerprint: "a".repeat(64),
            draft_id: "11111111-1111-4111-8111-111111111111".into(),
            generation: 1,
        },
        deposit_id: "22222222-2222-4222-8222-222222222222".into(),
        app_version: "0.1.0".into(),
        created_at_utc: "2026-09-14T01:02:03.004Z".into(),
        originals: vec![],
        attempt: None,
        draft: Draft::Template {
            sections: vec![],
            template: None,
            name: "PRIVATE 원문".into(),
            glossary_excluded: false,
            presentation: Intent::Keep,
            composing: true,
            fields: vec![DraftField {
                archive_title: None,
                writing_guide: None,
                id: "raw invalid field id".into(),
                label: "미확정".into(),
                configuration: DraftConfiguration::Number {
                    minimum: None,
                    maximum: None,
                },
                required: true,
                presentation: Intent::Unset,
                default: Intent::Set(ValueDto::Number { value: "-".into() }),
                archived: false,
                restore: false,
                archive_index: None,
                archive_order: vec![],
            }],
        },
    }
}

#[test]
fn fresh_app_local_store_initializes_nested_missing_parents_and_reopens_raw() {
    let fixture = Fixture::new();
    let root = fixture
        .0
        .join("app-local")
        .join("com.dreamrugi.worldbuildtool")
        .join("edit-recovery");
    assert!(!root.parent().unwrap().exists());
    let deposit = Deposit::freeze(sample()).unwrap();
    let mut store = Store::open(&root).unwrap();
    let proof = store.accept(&deposit).unwrap();
    assert!(proof.matches(
        deposit.key(),
        &deposit.envelope().deposit_id,
        deposit.payload_digest()
    ));
    drop(store);
    let store = Store::open(&root).unwrap();
    assert!(
        store
            .read(deposit.key(), &deposit.envelope().deposit_id)
            .unwrap()
            .envelope()
            == deposit.envelope()
    );
}

#[test]
fn document_term_info_round_trips_in_recovery_without_a_korean_name_copy() {
    let draft = Draft::Document {
        document: Some("33333333-3333-4333-8333-333333333333".into()),
        template: "44444444-4444-4444-8444-444444444444".into(),
        name: Intent::Set("아린".into()),
        english_name: Intent::Set("Arin".into()),
        glossary_summary: Intent::Set("북부 왕국의 기록관".into()),
        glossary_excluded: Intent::Keep,
        fields: vec![],
        composing: false,
    };
    let encoded = serde_json::to_vec(&draft).expect("encode recovery draft");
    let decoded: Draft = serde_json::from_slice(&encoded).expect("decode recovery draft");
    assert!(decoded == draft);
    let value: serde_json::Value = serde_json::from_slice(&encoded).expect("JSON value");
    assert_eq!(value["name"]["value"], "아린");
    assert_eq!(value["english_name"]["value"], "Arin");
    assert_eq!(value["glossary_summary"]["value"], "북부 왕국의 기록관");
    assert!(value.get("koreanName").is_none());
}

#[test]
#[ignore = "explicit local AppData verification; preserves the isolated verification Store"]
fn fresh_native_app_local_store_probe() {
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap())
        .join("com.dreamrugi.worldbuildtool.m34verification")
        .join("edit-recovery");
    assert!(
        !root.exists(),
        "fresh test requires an absent verification Store"
    );
    let envelope = sample();
    let deposit = Deposit::freeze(envelope.clone()).unwrap();
    let mut store = Store::open(&root).unwrap_or_else(|e| {
        panic!(
            "fresh Store: {:?}/{:?}, private cause: {:?}",
            e.category, e.stage, e.source
        );
    });
    let proof = store.accept(&deposit).unwrap();
    assert!(proof.matches(
        deposit.key(),
        &deposit.envelope().deposit_id,
        deposit.payload_digest()
    ));
    drop(store);
    let store = Store::open(&root).unwrap();
    assert!(
        store
            .read(deposit.key(), &deposit.envelope().deposit_id)
            .unwrap()
            .envelope()
            == &envelope
    );
}

#[test]
#[ignore = "temporary M7 E installed data path probe"]
fn m7_e_installed_store_path_probe() {
    let root = PathBuf::from(std::env::var_os("WORLDBUILD_E_PROBE_ROOT").unwrap());
    let physical_parent = std::fs::canonicalize(root.parent().unwrap()).unwrap();
    let physical = physical_parent.join("edit-recovery");
    match Store::open(&physical) {
        Ok(_) => println!("M7 E physical Store open passed"),
        Err(e) => panic!(
            "M7 E Store open: {:?}/{:?} source: {:?}",
            e.category, e.stage, e.source
        ),
    }
}
fn file(root: &Path, key: &Key) -> PathBuf {
    root.join(&key.project_fingerprint)
        .join(&key.draft_id)
        .join(format!("{}.json", key.generation))
}

#[test]
fn legacy_store_handoff_preserves_source_and_retries_idempotently() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let deposit = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&deposit).unwrap();
    let old_bytes = fs::read(file(&old_root, deposit.key())).unwrap();
    let mut target = Store::open(&new_root).unwrap();
    let first = target.import_legacy(&old_root).unwrap().unwrap();
    assert_eq!(
        (first.found, first.imported, first.needs_attention),
        (1, 1, 0)
    );
    assert_eq!(fs::read(file(&new_root, deposit.key())).unwrap(), old_bytes);
    assert_eq!(fs::read(file(&old_root, deposit.key())).unwrap(), old_bytes);
    let repeat = target.import_legacy(&old_root).unwrap().unwrap();
    assert_eq!(
        (
            repeat.imported,
            repeat.already_present,
            repeat.needs_attention
        ),
        (0, 1, 0)
    );
}

#[test]
fn startup_handoff_preserves_draft_without_opening_recovery_center() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let deposit = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&deposit).unwrap();
    let old_bytes = fs::read(file(&old_root, deposit.key())).unwrap();

    // This is the same synchronous entry used by Tauri setup. There is no
    // RecoveryPage call before the owner is dropped (normal home exit).
    let owner = Owner::with_legacy(new_root.clone(), Some(old_root.clone()));
    let summary = owner.handoff_legacy().unwrap().unwrap();
    assert_eq!((summary.imported, summary.needs_attention), (1, 0));
    assert_eq!(owner.handoff_status().state, "preserved");
    drop(owner);

    assert_eq!(fs::read(file(&new_root, deposit.key())).unwrap(), old_bytes);
    assert_eq!(fs::read(file(&old_root, deposit.key())).unwrap(), old_bytes);
    let next = Owner::with_legacy(new_root, Some(old_root));
    let retry = next.handoff_legacy().unwrap().unwrap();
    assert_eq!((retry.imported, retry.already_present), (0, 1));
}

#[test]
fn startup_handoff_with_no_old_store_does_not_create_a_target() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let owner = Owner::with_legacy(new_root.clone(), Some(old_root));
    assert!(owner.handoff_legacy().unwrap().is_none());
    assert_eq!(owner.handoff_status().state, "absent");
    assert!(!new_root.exists());
}

#[test]
fn startup_handoff_reports_a_corrupt_source_as_unpreserved() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let deposit = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&deposit).unwrap();
    fs::write(file(&old_root, deposit.key()), b"corrupt source").unwrap();
    let owner = Owner::with_legacy(new_root.clone(), Some(old_root.clone()));
    let summary = owner.handoff_legacy().unwrap().unwrap();
    assert_eq!((summary.imported, summary.needs_attention), (0, 1));
    assert_eq!(owner.handoff_status().state, "needs_attention");
    assert_eq!(
        fs::read(file(&old_root, deposit.key())).unwrap(),
        b"corrupt source"
    );
    assert!(!file(&new_root, deposit.key()).exists());
}

#[test]
fn startup_handoff_failure_is_visible_and_next_start_retries() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    fs::create_dir_all(old_root.parent().unwrap()).unwrap();
    fs::write(&old_root, b"invalid legacy store root").unwrap();
    let owner = Owner::with_legacy(new_root.clone(), Some(old_root.clone()));
    assert!(owner.handoff_legacy().is_err());
    assert_eq!(owner.handoff_status().state, "failed");
    assert!(owner.handoff_status().error.is_some());
    assert_eq!(fs::read(&old_root).unwrap(), b"invalid legacy store root");
    drop(owner);

    fs::remove_file(&old_root).unwrap();
    let deposit = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&deposit).unwrap();
    let next = Owner::with_legacy(new_root.clone(), Some(old_root));
    assert_eq!(next.handoff_legacy().unwrap().unwrap().imported, 1);
    assert_eq!(next.handoff_status().state, "preserved");
    assert_eq!(
        fs::read(file(&new_root, deposit.key())).unwrap(),
        deposit.bytes()
    );
}

#[test]
fn legacy_store_handoff_keeps_both_versions_on_conflict() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let original = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&original).unwrap();
    let mut changed = sample();
    if let Draft::Template { name, .. } = &mut changed.draft {
        *name = "다른 내용".into();
    }
    let changed = Deposit::freeze(changed).unwrap();
    let mut target = Store::open(&new_root).unwrap();
    target.accept(&changed).unwrap();
    let summary = target.import_legacy(&old_root).unwrap().unwrap();
    assert_eq!(
        (summary.found, summary.imported, summary.needs_attention),
        (1, 0, 1)
    );
    assert_eq!(
        fs::read(file(&old_root, original.key())).unwrap(),
        original.bytes()
    );
    assert_eq!(
        fs::read(file(&new_root, changed.key())).unwrap(),
        changed.bytes()
    );
}

#[test]
fn legacy_store_handoff_preserves_corrupt_source_for_retry() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let deposit = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&deposit).unwrap();
    fs::write(file(&old_root, deposit.key()), b"invalid old bytes").unwrap();
    let mut target = Store::open(&new_root).unwrap();
    let summary = target.import_legacy(&old_root).unwrap().unwrap();
    assert_eq!(summary.imported, 0);
    assert_eq!(summary.needs_attention, 1);
    assert_eq!(
        fs::read(file(&old_root, deposit.key())).unwrap(),
        b"invalid old bytes"
    );
    assert!(!file(&new_root, deposit.key()).exists());
}

#[test]
fn legacy_store_handoff_keeps_source_when_target_write_is_blocked() {
    let fixture = Fixture::new();
    let old_root = fixture.0.join("old-package").join("edit-recovery");
    let new_root = fixture.0.join("outside-package").join("edit-recovery");
    let deposit = Deposit::freeze(sample()).unwrap();
    Store::open(&old_root).unwrap().accept(&deposit).unwrap();
    let old_bytes = fs::read(file(&old_root, deposit.key())).unwrap();
    let mut target = Store::open(&new_root).unwrap();
    let target_file = file(&new_root, deposit.key());
    fs::create_dir_all(&target_file).unwrap();
    let summary = target.import_legacy(&old_root).unwrap().unwrap();
    assert_eq!((summary.imported, summary.needs_attention), (0, 1));
    assert_eq!(fs::read(file(&old_root, deposit.key())).unwrap(), old_bytes);
    assert!(target_file.is_dir());
}

#[test]
fn recovery_generation_scan_reserves_corrupt_names_and_rejects_incomplete_enumeration() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    let deposit = Deposit::freeze(sample()).unwrap();
    store.accept(&deposit).unwrap();
    let path = file(&fixture.root(), deposit.key());
    let before = fs::read(&path).unwrap();
    let dir = path.parent().unwrap();
    fs::write(dir.join("9007199254740993.json"), b"corrupt").unwrap();
    assert_eq!(
        store.latest_generation(deposit.key()).unwrap(),
        9007199254740993
    );
    // 지원 한도를 넘으면 선택+1이나 잘린 최대값으로 진행하지 않는다.
    for i in 0..MAX_LIST_ENTRIES {
        fs::write(dir.join(format!(".test-{i}.tmp")), []).unwrap();
    }
    let error = store.latest_generation(deposit.key()).unwrap_err();
    assert_eq!(error.category, Category::TooLarge);
    assert_eq!(error.stage, Stage::List);
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn recovery_roundtrip_invalid_input_idempotence_conflict_and_generation() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    let deposit = Deposit::freeze(sample()).unwrap();
    let proof = store.accept(&deposit).unwrap();
    assert!(proof.matches(
        deposit.key(),
        &deposit.envelope().deposit_id,
        deposit.payload_digest()
    ));
    let before = fs::read(file(&fixture.root(), deposit.key())).unwrap();
    assert_eq!(proof, store.accept(&deposit).unwrap());
    assert!(
        store
            .read(deposit.key(), &deposit.envelope().deposit_id)
            .unwrap()
            .envelope()
            == deposit.envelope()
    );
    let mut changed = sample();
    changed.app_version = "0.1.1".into();
    assert_eq!(
        store
            .accept(&Deposit::freeze(changed).unwrap())
            .unwrap_err()
            .category,
        Category::Conflict
    );
    assert_eq!(
        before,
        fs::read(file(&fixture.root(), deposit.key())).unwrap()
    );
    let mut later = sample();
    later.key.generation = 3;
    later.deposit_id = uuid::Uuid::new_v4().to_string();
    store.accept(&Deposit::freeze(later).unwrap()).unwrap();
    let mut stale = sample();
    stale.key.generation = 2;
    assert_eq!(
        store
            .accept(&Deposit::freeze(stale).unwrap())
            .unwrap_err()
            .category,
        Category::StaleGeneration
    );
    assert_eq!(store.list().unwrap().entries.len(), 2);
    assert!(!format!("{deposit:?}").contains("PRIVATE"));
}
#[test]
fn recovery_io_failures_keep_primary_cleanup_and_uncertainty() {
    for stage in [
        Stage::Create,
        Stage::Write,
        Stage::Flush,
        Stage::Sync,
        Stage::Publish,
        Stage::Reopen,
        Stage::Revalidate,
    ] {
        let fixture = Fixture::new();
        let mut store = Store::open(&fixture.root()).unwrap();
        store.fault = Some(stage);
        store.cleanup_fault = stage == Stage::Write;
        let deposit = Deposit::freeze(sample()).unwrap();
        let error = store.accept(&deposit).unwrap_err();
        assert_eq!(error.stage, stage);
        assert_eq!(error.cleanup.is_some(), stage == Stage::Write);
        let published = matches!(stage, Stage::Reopen | Stage::Revalidate);
        assert_eq!(error.published, published);
        assert_eq!(file(&fixture.root(), deposit.key()).exists(), published);
        if published {
            assert_eq!(error.category, Category::DurabilityUncertain);
            store.fault = Some(Stage::Revalidate);
            assert_eq!(
                store.accept(&deposit).unwrap_err().category,
                Category::DurabilityUncertain
            );
        }
        store.fault = None;
        store.cleanup_fault = false;
        assert!(store.accept(&deposit).is_ok());
        assert!(
            store
                .read(deposit.key(), &deposit.envelope().deposit_id)
                .unwrap()
                .envelope()
                == deposit.envelope()
        );
    }
    for (kind, category) in [
        (std::io::ErrorKind::PermissionDenied, Category::Permission),
        (std::io::ErrorKind::StorageFull, Category::Capacity),
    ] {
        let mut error = RecoveryError::io(
            Stage::Write,
            std::io::Error::new(kind, "PRIVATE C:\\secret"),
        );
        error.cleanup = Some(std::io::Error::other("PRIVATE cleanup"));
        assert_eq!(error.category, category);
        assert!(!format!("{error:?} {error}").contains("PRIVATE"));
        assert_eq!(
            error.source.as_ref().unwrap().to_string(),
            "PRIVATE C:\\secret"
        );
    }
}
#[test]
fn recovery_reader_isolates_future_corrupt_depth_size_id_and_digest() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    let deposit = Deposit::freeze(sample()).unwrap();
    store.accept(&deposit).unwrap();
    let path = file(&fixture.root(), deposit.key());
    let parent = path.parent().unwrap();
    let mut future: serde_json::Value = serde_json::from_slice(deposit.bytes()).unwrap();
    future["recoverySchemaVersion"] = 6.into();
    fs::write(parent.join("2.json"), serde_json::to_vec(&future).unwrap()).unwrap();
    fs::write(parent.join("3.json"), b"{broken").unwrap();
    fs::write(
        parent.join("4.json"),
        format!("{{\"deep\":{}{}}}", "[".repeat(130), "]".repeat(130)),
    )
    .unwrap();
    let large = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(parent.join("5.json"))
        .unwrap();
    large.set_len(MAX_FILE_BYTES as u64 + 1).unwrap();
    drop(large);
    fs::write(parent.join("bad.json"), b"{}").unwrap();
    let mut bad: serde_json::Value = serde_json::from_slice(deposit.bytes()).unwrap();
    bad["payloadDigest"] = "b".repeat(64).into();
    fs::write(parent.join("6.json"), serde_json::to_vec(&bad).unwrap()).unwrap();
    let mut wrong: serde_json::Value = serde_json::from_slice(deposit.bytes()).unwrap();
    wrong["envelope"]["key"]["draftId"] = "../escape".into();
    fs::write(parent.join("7.json"), serde_json::to_vec(&wrong).unwrap()).unwrap();
    fs::write(
        parent.join("8.json"),
        b"{\"recoverySchemaVersion\":1,\"recoverySchemaVersion\":1}",
    )
    .unwrap();
    fs::write(parent.join("9.json"), deposit.bytes()).unwrap(); // path/body identity mismatch
    let listing = store.list().unwrap();
    assert!(listing.complete);
    assert_eq!(listing.entries.len(), 10);
    assert_eq!(
        listing.entries.iter().filter(|r| r.error.is_none()).count(),
        1
    );
    for category in [
        Category::UnsupportedVersion,
        Category::Corrupt,
        Category::TooDeep,
        Category::TooLarge,
        Category::InvalidId,
        Category::DigestMismatch,
        Category::InvalidEnvelope,
    ] {
        assert!(
            listing
                .entries
                .iter()
                .any(|r| r.error.as_ref().is_some_and(|e| e.category == category)),
            "{category:?}"
        );
    }
    assert_eq!(fs::read_dir(parent).unwrap().count(), 10);
    assert!(!serde_json::to_string(&listing).unwrap().contains("PRIVATE"));
}
#[test]
fn recovery_root_exclusion_alias_and_no_replace_native() {
    let fixture = Fixture::new();
    let mut store = Store::open(&fixture.root()).unwrap();
    assert_eq!(
        Store::open(&fixture.root()).err().unwrap().category,
        Category::Busy
    );
    let busy = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD, "--ignored"])
        .env("WB_RECOVERY_CHILD_ROOT", fixture.root())
        .env("WB_RECOVERY_CHILD_MODE", "busy")
        .output()
        .unwrap();
    assert!(busy.status.success());
    assert!(fs::rename(fixture.root(), fixture.0.join("moved")).is_err());
    let deposit = Deposit::freeze(sample()).unwrap();
    store.accept(&deposit).unwrap();
    let path = file(&fixture.root(), deposit.key());
    let parent = path.parent().unwrap();
    let temp = native::open(&parent.join(".probe.tmp"), true, true).unwrap();
    assert!(native::publish(&temp, "1.json").is_err());
    native::cleanup(&temp).unwrap();
    drop(temp);
    let outside = fixture.0.join("external");
    drop(store);
    fs::hard_link(&path, &outside).unwrap();
    let mut store = Store::open(&fixture.root()).unwrap();
    assert_eq!(
        store.accept(&deposit).unwrap_err().category,
        Category::UnsafePath
    );
    fs::remove_file(outside).unwrap();
    let mut escape = sample();
    escape.key.draft_id = "../escape".into();
    assert_eq!(
        Deposit::freeze(escape).unwrap_err().category,
        Category::InvalidId
    );
    drop(store);
    assert!(Store::open(&fixture.root()).is_ok());
}
#[test]
fn recovery_junction_and_concurrent_shared_store() {
    let fixture = Fixture::new();
    let outside = fixture.0.join("outside");
    fs::create_dir(&outside).unwrap();
    let junction = fixture.0.join("junction");
    let result = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(result.status.success(), "junction fixture creation");
    assert!(Store::open(&junction.join("edit-recovery")).is_err());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    fs::remove_dir(&junction).unwrap();
    let store = Arc::new(Mutex::new(Store::open(&fixture.root()).unwrap()));
    let joins = (0..4)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .lock()
                    .unwrap()
                    .accept(&Deposit::freeze(sample()).unwrap())
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    let proofs = joins
        .into_iter()
        .map(|j| j.join().unwrap())
        .collect::<Vec<_>>();
    assert!(proofs.windows(2).all(|p| p[0] == p[1]));
    assert_eq!(store.lock().unwrap().list().unwrap().entries.len(), 1);
}
#[test]
fn recovery_limits_reject_without_truncation_and_bound_empty_directories() {
    let mut envelope = sample();
    if let Draft::Template { name, .. } = &mut envelope.draft {
        *name = "x".repeat(MAX_DRAFT_BYTES);
    }
    assert_eq!(
        Deposit::freeze(envelope).unwrap_err().category,
        Category::TooLarge
    );
    let fixture = Fixture::new();
    let store = Store::open(&fixture.root()).unwrap();
    for n in 0..MAX_LIST_ENTRIES + 1 {
        fs::create_dir(fixture.root().join(format!("{n:064x}"))).unwrap();
    }
    let listing = store.list().unwrap();
    assert!(!listing.complete);
    assert_eq!(listing.visited_nodes, MAX_LIST_ENTRIES);
}

#[test]
fn recovery_schema_header_generation_and_typed_depth_boundaries() {
    let mut envelope = sample();
    envelope.key.generation = 9_007_199_254_740_993;
    envelope.attempt = Some(Attempt {
        submitted_generation: envelope.key.generation,
        operation_id: uuid::Uuid::new_v4().to_string(),
        result: SaveState::Uncertain,
        candidate_digest: Some("b".repeat(64)),
        transaction_id: Some(format!("txn-{}", "c".repeat(64))),
    });
    let deposit = Deposit::freeze(envelope).unwrap();
    let value: serde_json::Value = serde_json::from_slice(deposit.bytes()).unwrap();
    assert_eq!(value["envelope"]["key"]["generation"], "9007199254740993");
    assert!(Deposit::decode(deposit.bytes()).unwrap().envelope() == deposit.envelope());
    for (pointer, replacement, category) in [
        (
            "/envelope/key/generation",
            serde_json::json!("01"),
            Category::Corrupt,
        ),
        (
            "/envelope/createdAtUtc",
            serde_json::json!("2026-09-14T01:02:03Z"),
            Category::InvalidEnvelope,
        ),
        (
            "/envelope/draft/kind",
            serde_json::json!("future_asset"),
            Category::UnsupportedKind,
        ),
    ] {
        let mut changed = value.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement;
        assert_eq!(
            Deposit::decode(&serde_json::to_vec(&changed).unwrap())
                .unwrap_err()
                .category,
            category
        );
    }
    let mut node = crate::data::edit_input::RichNode::Text {
        text: String::new(),
        marks: vec![],
    };
    for _ in 0..49 {
        node = crate::data::edit_input::RichNode::Blockquote {
            children: vec![node],
        };
    }
    let mut envelope = sample();
    if let Draft::Template { fields, .. } = &mut envelope.draft {
        fields[0].default = Intent::Set(ValueDto::RichText { content: node });
    }
    assert_eq!(
        Deposit::freeze(envelope).unwrap_err().category,
        Category::TooDeep
    );
}

#[test]
fn recovery_followup_keeps_actual_submitted_generation() {
    let mut e = sample();
    e.key.generation = 3;
    e.attempt = Some(Attempt {
        submitted_generation: 2,
        operation_id: uuid::Uuid::new_v4().to_string(),
        result: SaveState::NotApplied,
        candidate_digest: None,
        transaction_id: None,
    });
    let d = Deposit::freeze(e.clone()).unwrap();
    assert_eq!(
        Deposit::decode(d.bytes())
            .unwrap()
            .envelope()
            .attempt
            .as_ref()
            .unwrap()
            .submitted_generation,
        2
    );
    for bad in [0, 4] {
        e.attempt.as_mut().unwrap().submitted_generation = bad;
        assert!(Deposit::freeze(e.clone()).is_err());
    }
}

const CHILD: &str = "data::edit_recovery::tests::recovery_process_child";
#[test]
#[ignore = "전용 parent test가 격리 fixture에서만 실행"]
fn recovery_process_child() {
    let root = PathBuf::from(std::env::var_os("WB_RECOVERY_CHILD_ROOT").unwrap());
    let mode = std::env::var("WB_RECOVERY_CHILD_MODE").unwrap();
    if mode == "busy" {
        assert_eq!(Store::open(&root).err().unwrap().category, Category::Busy);
        return;
    }
    let deposit = Deposit::freeze(sample()).unwrap();
    let mut store = Store::open(&root).unwrap();
    if mode == "read" {
        let expected = std::env::var("WB_RECOVERY_EXPECT").unwrap();
        let listing = store.list().unwrap();
        let ready = listing.entries.iter().filter(|e| e.error.is_none()).count();
        assert_eq!(ready, usize::from(expected != "before"));
        if ready == 1 {
            assert!(
                store
                    .read(deposit.key(), &deposit.envelope().deposit_id)
                    .unwrap()
                    .envelope()
                    == deposit.envelope()
            );
            assert!(store
                .revalidate(
                    deposit.key(),
                    &deposit.envelope().deposit_id,
                    deposit.payload_digest()
                )
                .is_ok());
        }
        return;
    }
    let signal = root.parent().unwrap().join("checkpoint");
    if mode != "receipt" {
        let target = if mode == "before" {
            Stage::Publish
        } else {
            Stage::Revalidate
        };
        store.hook = Some(Box::new(move |stage| {
            if stage == target {
                fs::write(&signal, b"boundary reached; no receipt").unwrap();
                loop {
                    std::thread::park();
                }
            }
        }));
    }
    let _proof = store.accept(&deposit).unwrap();
    fs::write(
        root.parent().unwrap().join("checkpoint"),
        b"receipt returned; response lost",
    )
    .unwrap();
    loop {
        std::thread::park();
    }
}
#[test]
fn recovery_real_process_termination_and_cold_read() {
    for mode in ["before", "published", "receipt"] {
        let fixture = Fixture::new();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("WB_RECOVERY_CHILD_ROOT", fixture.root())
            .env("WB_RECOVERY_CHILD_MODE", mode)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(25);
        while !fixture.0.join("checkpoint").exists() {
            if child.try_wait().unwrap().is_some() {
                panic!("recovery helper exited before boundary");
            }
            if std::time::Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("recovery helper deadline");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let read = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", CHILD, "--ignored", "--nocapture"])
            .env("WB_RECOVERY_CHILD_ROOT", fixture.root())
            .env("WB_RECOVERY_CHILD_MODE", "read")
            .env("WB_RECOVERY_EXPECT", mode)
            .output()
            .unwrap();
        assert!(
            read.status.success(),
            "cold reader failed: {}",
            String::from_utf8_lossy(&read.stderr)
        );
    }
}

#[test]
fn m36_recovery_v1_retains_original_bytes_and_rejects_v2_semantics() {
    let deposit = Deposit::freeze(sample()).unwrap();
    let bytes = String::from_utf8(deposit.bytes().to_vec())
        .unwrap()
        .replace("\"recoverySchemaVersion\":5", "\"recoverySchemaVersion\":1");
    let decoded = Deposit::decode(bytes.as_bytes()).unwrap();
    assert_eq!(decoded.bytes(), bytes.as_bytes());
    let mut e = sample();
    if let Draft::Template { fields, .. } = &mut e.draft {
        fields[0].default = Intent::Set(ValueDto::NumberUnknown {
            previous_raw: Some("-".into()),
        });
    }
    let deposit = Deposit::freeze(e).unwrap();
    let current = Deposit::decode(deposit.bytes()).unwrap();
    assert_eq!(current.bytes(), deposit.bytes());
    let legacy = String::from_utf8(deposit.bytes().to_vec())
        .unwrap()
        .replace("\"recoverySchemaVersion\":5", "\"recoverySchemaVersion\":1");
    assert!(Deposit::decode(legacy.as_bytes()).is_err());
}
