use super::*;
// An explicit, persistent native fixture accompanies the real owned NSIS update.
// This is a controlled attachment sample, separate from the GUI-entered draft.
#[cfg(all(windows, feature = "compatibility-test"))]
#[test]
fn m8_owned_install_attachment_capture_and_cold_read() {
    let base = PathBuf::from(
        std::env::var("M8_B_INSTALL_FIXTURE").expect("owned install fixture required"),
    );
    assert!(
        base.is_absolute()
            && base
                .file_name()
                .unwrap()
                .to_string_lossy()
                .eq_ignore_ascii_case("M8-B-INSTALL-TEST")
    );
    let proof_path = base.join("attachment-proof.json");
    let root = base.join("data/edit-recovery");
    if std::env::var("M8_B_ASSET_PHASE").unwrap() == "capture" {
        assert!(!proof_path.exists());
        let project = base.join("attachment-sample-project");
        fs::create_dir(&project).unwrap();
        let source = base.join("attachment-sample.txt");
        fs::write(
            &source,
            b"M8 B controlled attachment before actual NSIS update",
        )
        .unwrap();
        let asset = crate::data::assets::Store::open(&project, true)
            .unwrap()
            .import(&source, false, &uuid::Uuid::new_v4().to_string())
            .unwrap();
        let mut store = Store::open(&root).unwrap();
        let entries = store.list().unwrap();
        let actual = entries
            .entries
            .iter()
            .find(|entry| entry.key.is_some())
            .unwrap();
        let mut envelope = sample();
        envelope.key.project_fingerprint = actual.key.as_ref().unwrap().project_fingerprint.clone();
        envelope.key.draft_id = uuid::Uuid::new_v4().to_string();
        envelope.deposit_id = uuid::Uuid::new_v4().to_string();
        envelope.app_version = "0.2.1".into();
        envelope.created_at_utc = crate::data::utc_time::now_utc_milliseconds().unwrap();
        if let Draft::Template { name, fields, .. } = &mut envelope.draft {
            *name = "첨부 보관 통제 표본 B".into();
            fields[0].label = "시험 첨부".into();
            fields[0].configuration = DraftConfiguration::File {};
            fields[0].default = Intent::Set(ValueDto::File {
                value: vec![asset.id.clone()],
            });
        }
        let deposit = Deposit::freeze(envelope).unwrap();
        store.capture_assets(&deposit, &project).unwrap();
        let proof = store.accept(&deposit).unwrap();
        assert!(proof.matches(
            deposit.key(),
            &deposit.envelope().deposit_id,
            deposit.payload_digest()
        ));
        fs::write(&proof_path, serde_json::to_vec_pretty(&serde_json::json!({"key":deposit.key(),"depositId":deposit.envelope().deposit_id,"payloadDigest":deposit.payload_digest(),"assetId":asset.id,"sourceSHA256":model::digest(&fs::read(source).unwrap()),"controlledSample":true})).unwrap()).unwrap();
    } else {
        let proof: serde_json::Value =
            serde_json::from_slice(&fs::read(proof_path).unwrap()).unwrap();
        let key: Key = serde_json::from_value(proof["key"].clone()).unwrap();
        let mut store = Store::open(&root).unwrap();
        let cold = store
            .read(&key, proof["depositId"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            cold.payload_digest(),
            proof["payloadDigest"].as_str().unwrap()
        );
        store.accept(&cold).unwrap();
        let restored = base.join("attachment-restored-after-update");
        fs::create_dir(&restored).unwrap();
        store.restore_assets(&cold, &restored).unwrap();
        let (_, bytes) = crate::data::assets::Store::open(&restored, false)
            .unwrap()
            .read(proof["assetId"].as_str().unwrap())
            .unwrap();
        assert_eq!(
            model::digest(&bytes),
            proof["sourceSHA256"].as_str().unwrap()
        );
    }
}
#[test]
fn m37_receipt_requires_bytes_and_cold_restore_survives_unavailable_project() {
    let f = Fixture::new();
    let project = f.0.join("project");
    fs::create_dir(&project).unwrap();
    let source = f.0.join("source.txt");
    fs::write(&source, b"owned recovery bytes").unwrap();
    let asset = crate::data::assets::Store::open(&project, true)
        .unwrap()
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let mut envelope = sample();
    if let Draft::Template { fields, .. } = &mut envelope.draft {
        fields[0].configuration = DraftConfiguration::File {};
        fields[0].default = Intent::Set(ValueDto::File {
            value: vec![asset.id.clone()],
        });
    }
    let deposit = Deposit::freeze(envelope).unwrap();
    let mut store = Store::open(&f.root()).unwrap();
    assert!(store.accept(&deposit).is_err());
    store.capture_assets(&deposit, &project).unwrap();
    let proof = store.accept(&deposit).unwrap();
    assert!(proof.matches(
        deposit.key(),
        &deposit.envelope().deposit_id,
        deposit.payload_digest()
    ));
    let listing = store.list().unwrap();
    assert!(listing.complete);
    assert_eq!(listing.entries.len(), 1);
    assert!(listing.entries[0].error.is_none());
    store
        .verify_handoff(deposit.key(), &deposit.envelope().deposit_id)
        .unwrap();
    drop(store);
    fs::rename(&project, f.0.join("unavailable-project")).unwrap();
    let mut store = Store::open(&f.root()).unwrap();
    let cold = store
        .read(deposit.key(), &deposit.envelope().deposit_id)
        .unwrap();
    store.accept(&cold).unwrap();
    let restored = f.0.join("restored-project");
    fs::create_dir(&restored).unwrap();
    store.restore_assets(&cold, &restored).unwrap();
    store.restore_assets(&cold, &restored).unwrap();
    let actual = crate::data::assets::Store::open(&restored, false)
        .unwrap()
        .read(&asset.id)
        .unwrap();
    assert_eq!(actual.0, asset);
    assert_eq!(actual.1, b"owned recovery bytes");
    let (path, guards) = store.directory(deposit.key(), false).unwrap();
    drop(guards);
    fs::write(
        path.join("assets").join(&asset.id).join("content.txt"),
        b"corrupt",
    )
    .unwrap();
    assert!(store.accept(&deposit).is_err());
    assert!(store.restore_assets(&cold, &restored).is_err());
    assert!(store
        .verify_handoff(deposit.key(), &deposit.envelope().deposit_id)
        .is_err());
    assert_eq!(fs::read(&source).unwrap(), b"owned recovery bytes");
}

#[test]
fn m8_asset_namespace_must_be_a_valid_directory_and_unknown_children_still_fail_listing() {
    let f = Fixture::new();
    let deposit = Deposit::freeze(sample()).unwrap();
    let mut store = Store::open(&f.root()).unwrap();
    store.accept(&deposit).unwrap();
    let (path, guards) = store.directory(deposit.key(), false).unwrap();
    drop(guards);
    fs::write(path.join("assets"), b"not a directory").unwrap();
    assert!(store
        .list()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.error.is_some()));
    fs::remove_file(path.join("assets")).unwrap();
    fs::create_dir(path.join("unrecognized")).unwrap();
    assert!(store
        .list()
        .unwrap()
        .entries
        .iter()
        .any(|entry| entry.error.is_some()));
}

#[test]
fn m545_recovery_captures_exact_referenced_asset_from_project_trash() {
    let f = Fixture::new();
    let project = f.0.join("project");
    fs::create_dir(&project).unwrap();
    let source = f.0.join("attachment.txt");
    fs::write(&source, b"trashed attachment bytes").unwrap();
    let asset = crate::data::assets::Store::open(&project, true)
        .unwrap()
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let trash = project.join("assets/.trash");
    fs::create_dir(&trash).unwrap();
    fs::rename(
        project.join("assets").join(&asset.id),
        trash.join(&asset.id),
    )
    .unwrap();

    let mut envelope = sample();
    if let Draft::Template { fields, .. } = &mut envelope.draft {
        fields[0].configuration = DraftConfiguration::File {};
        fields[0].default = Intent::Set(ValueDto::File {
            value: vec![asset.id.clone()],
        });
    }
    let deposit = Deposit::freeze(envelope).unwrap();
    let mut store = Store::open(&f.root()).unwrap();
    store.capture_assets(&deposit, &project).unwrap();
    let proof = store.accept(&deposit).unwrap();
    assert!(proof.matches(
        deposit.key(),
        &deposit.envelope().deposit_id,
        deposit.payload_digest()
    ));

    let restored = f.0.join("restored");
    fs::create_dir(&restored).unwrap();
    store.restore_assets(&deposit, &restored).unwrap();
    let actual = crate::data::assets::Store::open(&restored, false)
        .unwrap()
        .read(&asset.id)
        .unwrap();
    assert_eq!(actual.0, asset);
    assert_eq!(actual.1, b"trashed attachment bytes");
    assert!(!project.join("assets").join(&asset.id).exists());
    assert!(trash.join(&asset.id).exists());
}
#[test]
fn m37_recovery_media_cannot_be_read_under_older_version_headers() {
    let mut envelope = sample();
    if let Draft::Template { fields, .. } = &mut envelope.draft {
        fields[0].configuration = DraftConfiguration::Url {};
        fields[0].default = Intent::Set(ValueDto::Url {
            value: "invalid raw URL".into(),
        });
    }
    let deposit = Deposit::freeze(envelope).unwrap();
    let raw = String::from_utf8(deposit.bytes().to_vec()).unwrap();
    for version in [1, 2] {
        assert!(Deposit::decode(
            raw.replace(
                "\"recoverySchemaVersion\":5",
                &format!("\"recoverySchemaVersion\":{version}")
            )
            .as_bytes()
        )
        .is_err());
    }
    assert_eq!(
        Deposit::decode(deposit.bytes()).unwrap().bytes(),
        deposit.bytes()
    );
}

#[test]
fn m38_group_receipt_and_subprocess_restore_preserve_raw_order_and_shared_bytes() {
    use crate::data::artifact;
    use serde_json::json;
    const CHILD: &str = "WB_M38_RECOVERY_CHILD";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let expected = Deposit::decode(&fs::read(root.join("expected.json")).unwrap()).unwrap();
        let mut store = Store::open(&root.join("edit-recovery")).unwrap();
        let cold = store
            .read(expected.key(), &expected.envelope().deposit_id)
            .unwrap();
        assert_eq!(cold.bytes(), expected.bytes());
        store.accept(&cold).unwrap();
        let restored = root.join("restored");
        fs::create_dir(&restored).unwrap();
        store.restore_assets(&cold, &restored).unwrap();
        let id = fs::read_to_string(root.join("asset-id")).unwrap();
        assert_eq!(
            crate::data::assets::Store::open(&restored, false)
                .unwrap()
                .read(&id)
                .unwrap()
                .1,
            b"shared group recovery bytes"
        );
        fs::write(root.join("child-pass"), b"restored").unwrap();
        return;
    }
    let f = Fixture::new();
    let project = f.0.join("project");
    fs::create_dir(&project).unwrap();
    let source = f.0.join("source.txt");
    fs::write(&source, b"shared group recovery bytes").unwrap();
    let asset = crate::data::assets::Store::open(&project, true)
        .unwrap()
        .import(&source, false, &uuid::Uuid::new_v4().to_string())
        .unwrap();
    let template = artifact::create_template(
        "group recovery".into(),
        None,
        "2026-09-17T01:02:03.004Z".into(),
    )
    .unwrap();
    let snapshot = String::from_utf8(artifact::encode_template(&template).unwrap()).unwrap();
    let mut e = sample();
    e.originals = vec![Original {
        kind: OriginalKind::Template,
        artifact_id: template.template_id().to_string(),
        schema: template.schema_version().get(),
        template_revision: template.revision().get(),
        source_byte_length: snapshot.len() as u64,
        source_digest: digest(snapshot.as_bytes()),
        snapshot_digest: digest(snapshot.as_bytes()),
        snapshot,
    }];
    e.draft = Draft::Document { document: None, template: template.template_id().to_string(), name: Intent::Set("raw group draft".into()), english_name: Intent::Keep, glossary_summary: Intent::Keep, glossary_excluded: Intent::Keep, composing: false, fields: vec![DraftValue { field: uuid::Uuid::new_v4().to_string(), value: Intent::Set(serde_json::from_value(json!({"kind":"group","instances":[
      {"id":"bbbbbbbb-bbbb-4bbb-8bbb-000000000002","source":null,"fields":[{"field":"number","value":{"intent":"set","value":{"kind":"number","value":"-"}}},{"field":"file","value":{"intent":"set","value":{"kind":"file","value":[asset.id]}}}]},
      {"id":"bbbbbbbb-bbbb-4bbb-8bbb-000000000001","source":null,"fields":[{"field":"number","value":{"intent":"set","value":{"kind":"number_unknown","previous_raw":"0"}}},{"field":"file","value":{"intent":"set","value":{"kind":"file","value":[asset.id]}}}]}
    ]})).unwrap()) }] };
    let deposit = Deposit::freeze(e).unwrap();
    let mut store = Store::open(&f.root()).unwrap();
    assert!(store.accept(&deposit).is_err());
    store.capture_assets(&deposit, &project).unwrap();
    store.accept(&deposit).unwrap();
    fs::write(f.0.join("expected.json"), deposit.bytes()).unwrap();
    fs::write(f.0.join("asset-id"), &asset.id).unwrap();
    drop(store);
    fs::rename(project, f.0.join("unavailable-project")).unwrap();
    let child = Command::new(std::env::current_exe().unwrap()).args(["--exact", "data::edit_recovery::tests::assets::m38_group_receipt_and_subprocess_restore_preserve_raw_order_and_shared_bytes", "--nocapture"]).env(CHILD, &f.0).output().unwrap();
    assert!(
        child.status.success(),
        "{} {}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(fs::read(f.0.join("child-pass")).unwrap(), b"restored");
    let raw = String::from_utf8(deposit.bytes().to_vec()).unwrap();
    for version in [1, 2, 3] {
        assert!(Deposit::decode(
            raw.replace(
                "\"recoverySchemaVersion\":5",
                &format!("\"recoverySchemaVersion\":{version}")
            )
            .as_bytes()
        )
        .is_err());
    }
}
