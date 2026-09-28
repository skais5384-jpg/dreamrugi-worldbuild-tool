// G4의 실제 G2 → repository → collaboration permit → M1 경계를 검증한다.
#[cfg(windows)]
#[path = "write_ownership_tests.rs"]
mod b003_connection;
#[path = "write_contract_tests.rs"]
mod contracts;
use super::*;
use crate::data::{
    collaboration_lock::{LockCoordinator, LockService, LockSessionId, NoLockService},
    project_runtime::{ProjectRuntime, RuntimeState},
    storage_estimate::{
        test_support::{with_storage_response, TestStorageResponse},
        StorageQueryErrorKind,
    },
    transaction::{
        prepare_canonical_with_hooks, CommitResultState, PrepareFailPoint, PrepareHooks,
        PrepareStage,
    },
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
    sync::Arc,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn writing_guide_schema_upgrade_uses_exact_backup_and_existing_recovery() -> TestResult {
    use artifact::template_mutation::{
        whole::{prepare_template_draft, FieldDraftInput, TemplateDraftInput},
        FieldValueDraft, NewFieldConfiguration,
    };
    for commit in [false, true] {
        let fixture = Fixture::new();
        let old = raw(&template_value());
        fixture.write(ArtifactSourceId::Template(template_id()), &old);
        let mut runtime = fixture.runtime();
        let expected;
        {
            let ready = runtime.ready()?;
            let repo = ArtifactRepository::new(&ready)?;
            let loaded = repo.load_template(template_id())?;
            let candidate = prepare_template_draft(
                loaded.artifact(),
                loaded.artifact().revision(),
                TIME,
                TemplateDraftInput {
                    sections: vec![],
                    name: loaded.artifact().name().into(),
                    glossary_excluded: loaded.artifact().glossary_excluded(),
                    presentation_token: None,
                    fields: vec![FieldDraftInput {
                        members: vec![],
                        id: FIELD.parse()?,
                        label: "인구".into(),
                        kind: artifact::FieldKind::Number,
                        configuration: NewFieldConfiguration::number(),
                        required: true,
                        writing_guide: Some("예: 1000".into()),
                        presentation_token: None,
                        default: Some(FieldValueDraft::unset()),
                        archived: false,
                        archived_options: Default::default(),
                    }],
                },
            )?
            .into_changed()
            .unwrap();
            expected = artifact::encode_template(&candidate)?;
            let plan = CanonicalWritePlan::new().replace_template(&candidate, loaded.source())?;
            with_plan(plan, &repo, |plan, permit| -> TestResult {
                let prepared = plan.prepare(&repo, permit)?;
                let inner = prepared.inner_for_test();
                let op = &inner.manifest().operations[0];
                assert_eq!(op.original_schema_version.unwrap().get(), 1);
                assert_eq!(op.staged_schema_version.unwrap().get(), 2);
                assert_eq!(
                    fs::read(
                        inner
                            .transaction_directory()
                            .join(op.backup_path.as_ref().unwrap())
                    )?,
                    old
                );
                assert_eq!(
                    fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
                    old
                );
                if commit {
                    assert_eq!(
                        prepared.commit()?.result_state(),
                        CommitResultState::Committed
                    );
                }
                Ok(())
            })?;
        }
        if !commit {
            assert_eq!(runtime.recover()?.rolled_back_transactions, 1);
        }
        assert_eq!(
            fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
            if commit { expected } else { old }
        );
    }
    Ok(())
}

#[test]
fn m42_fix002_relation_name_schema_upgrade_commits_only_the_exact_v5_to_v6_document() -> TestResult
{
    let fixture = Fixture::new();
    let mut old_value = document_value();
    old_value["schemaVersion"] = 5.into();
    old_value["fieldValues"][FIELD] = json!({
        "kind":"relation",
        "links":[{"id":FIELD,"document":OTHER,"oneWay":false}]
    });
    let old = raw(&old_value);
    fixture.write(ArtifactSourceId::Document(document_id()), &old);

    let mut candidate_value = old_value;
    candidate_value["schemaVersion"] = 6.into();
    candidate_value["fieldValues"][FIELD]["links"][0]["name"] = json!("친구");
    let candidate = artifact::decode_document(&raw(&candidate_value))?;
    let expected = artifact::encode_document(&candidate)?;

    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let loaded = repo.load_document(document_id())?;
    let plan = CanonicalWritePlan::new().replace_document(&candidate, loaded.source())?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        let operation = &prepared.inner_for_test().manifest().operations[0];
        assert_eq!(operation.original_schema_version.unwrap().get(), 5);
        assert_eq!(operation.staged_schema_version.unwrap().get(), 6);
        assert_eq!(
            prepared.commit()?.result_state(),
            CommitResultState::Committed
        );
        Ok(())
    })?;
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?,
        expected
    );
    Ok(())
}
const TEMPLATE: &str = "11111111-1111-4111-8111-111111111111";
const DOCUMENT: &str = "22222222-2222-4222-8222-222222222222";
const OTHER: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const FIELD: &str = "33333333-3333-4333-8333-333333333333";
const TIME: &str = "2026-09-07T01:02:03.004Z";
const CANARY: &str =
    "G4_PRIVATE_CANARY_6f8b credential=secret C:/Users/private/body.json?token=private";
const LEXEMES: &str =
    r#"{"a":1E100,"b":1e100,"c":-0,"d":0.12345678901234567890123456789,"nested":[{"x":1E100}]}"#;
fn template_id() -> TemplateId {
    TEMPLATE.parse().unwrap()
}
fn document_id() -> DocumentId {
    DOCUMENT.parse().unwrap()
}
fn template_value() -> Value {
    json!({"artifactType":"template","schemaVersion":1,"templateId":TEMPLATE,"revision":7,
        "name":CANARY,"lifecycle":"active","presentation":{"future":null},"fieldOrder":[],"fields":{},
        "createdAtUtc":TIME,"updatedAtUtc":TIME,"future":null})
}
fn document_value() -> Value {
    json!({"artifactType":"document","schemaVersion":1,"documentId":DOCUMENT,"templateId":TEMPLATE,
        "templateRevision":7,"name":CANARY,"fieldValues":{},"orphanedFieldDefinitions":{},
        "createdAtUtc":TIME,"updatedAtUtc":TIME,"future":null})
}
fn raw(value: &Value) -> Vec<u8> {
    serde_json::to_string(value)
        .unwrap()
        .replace("\"future\":null", &format!("\"future\":{LEXEMES}"))
        .into_bytes()
}
fn template() -> TemplateArtifact {
    artifact::decode_template(&raw(&template_value())).unwrap()
}
fn document() -> DocumentArtifact {
    artifact::decode_document(&raw(&document_value())).unwrap()
}

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("worldbuild-g4-{}", uuid::Uuid::new_v4()));
        let root = base.join("project");
        fs::create_dir_all(root.join("templates")).unwrap();
        fs::create_dir(root.join("documents")).unwrap();
        Self { base, root }
    }
    fn runtime(&self) -> ProjectRuntime {
        let mut runtime = ProjectRuntime::acquire(&self.root, &self.base.join("locks")).unwrap();
        assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
        runtime.recover().unwrap();
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        runtime
    }
    fn path(&self, id: ArtifactSourceId) -> PathBuf {
        self.root.join(id.path().unwrap().as_str())
    }
    fn write(&self, id: ArtifactSourceId, bytes: &[u8]) {
        fs::write(self.path(id), bytes).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(e) = fs::remove_dir_all(&self.base) {
            eprintln!("G4 fixture cleanup: {:?}", e.kind());
        }
    }
}
fn inventory(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut result = BTreeMap::new();
    for entry in fs::read_dir(root).unwrap() {
        let entry = entry.unwrap();
        let meta = fs::symlink_metadata(entry.path()).unwrap();
        if meta.is_dir() && !project_file::directory::is_reparse(&meta) {
            result.insert(entry.path(), None);
            result.extend(inventory(&entry.path()));
        } else if meta.is_file() {
            result.insert(entry.path(), Some(fs::read(entry.path()).unwrap()));
        }
    }
    result
}
fn with_permit<T>(
    fingerprint: &str,
    targets: Vec<ProjectRelativePath>,
    run: impl FnOnce(WritePermit<'_>) -> T,
) -> T {
    let service: Arc<dyn LockService> = Arc::new(NoLockService::new());
    let coordinator = LockCoordinator::new(service);
    let mut guard = coordinator
        .acquire_all(fingerprint, &LockSessionId::generate().unwrap(), targets)
        .unwrap();
    let result = run(guard.write_permit().unwrap());
    guard.release_all().unwrap();
    result
}
fn with_plan<T>(
    plan: CanonicalWritePlan,
    repo: &ArtifactRepository<'_, '_>,
    run: impl FnOnce(CanonicalWritePlan, WritePermit<'_>) -> T,
) -> T {
    let targets = plan.targets().cloned().collect();
    with_permit(repo.write_project().fingerprint(), targets, |permit| {
        run(plan, permit)
    })
}
fn no_leak(value: &impl fmt::Debug) {
    let output = format!("{value:?}");
    for secret in [
        CANARY,
        "G4_PRIVATE_CANARY_6f8b",
        "credential=secret",
        "private/body.json",
        "1E100",
    ] {
        assert!(!output.contains(secret), "leaked diagnostic");
    }
}
fn checked_error(error: &ArtifactWriteError, category: ArtifactWriteCategory) {
    assert_eq!(error.diagnostic().category, category);
    no_leak(error);
    no_leak(error.diagnostic());
    assert!(!error.to_string().contains(CANARY));
    assert!(error.source().is_none());
    assert!(!error.diagnostic().next_action().is_empty());
}

#[test]
fn creates_both_kinds_in_one_plan_and_commits_exact_codec_bytes() -> TestResult {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let template = template();
    let mut value = document_value();
    value["fieldValues"][FIELD] = json!({"kind":"richText","document":{"schemaVersion":1,"content":{"kind":"root","children":[{"kind":"paragraph","children":[{"kind":"text","text":CANARY,"future":null}],"future":null}],"future":null}},"future":null});
    value["orphanedFieldDefinitions"][FIELD] =
        json!({"kind":"richText","label":CANARY,"options":{},"future":null});
    let document = artifact::decode_document(&raw(&value))?;
    let original_template = template.clone();
    let original_document = document.clone();
    let template_bytes = artifact::encode_template(&template)?;
    let document_bytes = artifact::encode_document(&document)?;
    for bytes in [&template_bytes, &document_bytes] {
        let text = String::from_utf8(bytes.clone())?;
        for lexeme in ["1E100", "1e100", "-0", "0.12345678901234567890123456789"] {
            assert!(text.contains(lexeme));
        }
        assert!(text.ends_with('\n'));
        assert!(!text.contains('\r'));
    }
    let before = inventory(&fixture.root);
    let plan = CanonicalWritePlan::new()
        .create_template(&template)?
        .create_document(&document)?;
    no_leak(&plan);
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        no_leak(&prepared);
        assert!(prepared.transaction_id().as_str().starts_with("txn-"));
        let inner = prepared.inner_for_test();
        assert_eq!(inner.manifest().operations.len(), 2);
        for operation in &inner.manifest().operations {
            let expected = if operation.target_path.as_str().starts_with("templates/") {
                &template_bytes
            } else {
                &document_bytes
            };
            assert!(!operation.original_existed);
            assert_eq!(operation.backup_path, None);
            assert_eq!(operation.staged_size, expected.len() as u64);
            assert_eq!(
                operation.staged_sha256,
                format!("{:x}", Sha256::digest(expected))
            );
            assert_eq!(operation.staged_schema_version.unwrap().get(), 1);
            assert_eq!(
                fs::read(inner.transaction_directory().join(&operation.staged_path))?,
                *expected
            );
            assert!(!fixture.root.join(operation.target_path.as_str()).exists());
        }
        assert!(
            prepared.storage_admission().required_peak_bytes()
                > (template_bytes.len() + document_bytes.len()) as u64
        );
        let outcome = prepared.commit()?;
        assert_eq!(
            outcome.result_state(),
            CommitResultState::Committed,
            "{outcome:?}"
        );
        no_leak(&outcome);
        Ok(())
    })?;
    assert_eq!(template, original_template);
    assert_eq!(document, original_document);
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
        template_bytes
    );
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?,
        document_bytes
    );
    assert_eq!(repo.load_template(template_id())?.artifact(), &template);
    assert_eq!(repo.load_document(document_id())?.artifact(), &document);
    assert!(before.keys().all(|path| path.exists()));
    Ok(())
}

#[test]
fn replaces_both_kinds_from_raw_source_with_new_revisions_and_exact_backups() -> TestResult {
    let fixture = Fixture::new();
    let t_raw = raw(&template_value());
    let d_raw = raw(&document_value());
    fixture.write(ArtifactSourceId::Template(template_id()), &t_raw);
    fixture.write(ArtifactSourceId::Document(document_id()), &d_raw);
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let t = repo.load_template(template_id())?;
    let d = repo.load_document(document_id())?;
    let mut tv = template_value();
    tv["revision"] = json!(8);
    tv["name"] = json!("new template");
    let mut dv = document_value();
    dv["templateRevision"] = json!(8);
    dv["name"] = json!("new document");
    let tc = artifact::decode_template(&raw(&tv))?;
    let dc = artifact::decode_document(&raw(&dv))?;
    let plan = CanonicalWritePlan::new()
        .replace_template(&tc, t.source())?
        .replace_document(&dc, d.source())?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        let inner = prepared.inner_for_test();
        for op in &inner.manifest().operations {
            let old = if op.target_path.as_str().starts_with("templates/") {
                &t_raw
            } else {
                &d_raw
            };
            assert!(op.original_existed);
            assert_eq!(op.original_size, Some(old.len() as u64));
            assert_eq!(
                op.original_sha256,
                Some(format!("{:x}", Sha256::digest(old)))
            );
            assert_eq!(op.original_schema_version, op.staged_schema_version);
            assert_eq!(op.original_schema_version.unwrap().get(), 1);
            assert_eq!(
                fs::read(
                    inner
                        .transaction_directory()
                        .join(op.backup_path.as_ref().unwrap())
                )?,
                *old
            );
            assert_eq!(fs::read(fixture.root.join(op.target_path.as_str()))?, *old);
        }
        assert_eq!(
            prepared.commit()?.result_state(),
            CommitResultState::Committed
        );
        Ok(())
    })?;
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
        artifact::encode_template(&tc)?
    );
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?,
        artifact::encode_document(&dc)?
    );
    assert_eq!(
        artifact::encode_template(t.artifact())?,
        artifact::encode_template(&template())?
    );
    assert_eq!(
        artifact::encode_document(d.artifact())?,
        artifact::encode_document(&document())?
    );
    Ok(())
}

#[test]
fn source_id_kind_and_duplicate_targets_are_rejected_before_io() -> TestResult {
    let fixture = Fixture::new();
    fixture.write(
        ArtifactSourceId::Template(template_id()),
        &raw(&template_value()),
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let source = repo.load_template(template_id())?;
    let before = inventory(&fixture.root);
    let mut other = template_value();
    other["templateId"] = json!(OTHER);
    checked_error(
        &CanonicalWritePlan::new()
            .replace_template(&artifact::decode_template(&raw(&other))?, source.source())
            .unwrap_err(),
        ArtifactWriteCategory::SourceMismatch,
    );
    checked_error(
        &CanonicalWritePlan::new()
            .replace_document(&document(), source.source())
            .unwrap_err(),
        ArtifactWriteCategory::SourceMismatch,
    );
    checked_error(
        &CanonicalWritePlan::new()
            .create_template(&template())?
            .create_template(&template())
            .unwrap_err(),
        ArtifactWriteCategory::DuplicateTarget,
    );
    assert_eq!(inventory(&fixture.root), before);
    Ok(())
}

#[test]
fn same_bytes_in_other_project_do_not_make_a_valid_source() -> TestResult {
    let a = Fixture::new();
    let b = Fixture::new();
    for f in [&a, &b] {
        f.write(
            ArtifactSourceId::Template(template_id()),
            &raw(&template_value()),
        );
    }
    let mut ra = a.runtime();
    let aa = ra.ready()?;
    let repo_a = ArtifactRepository::new(&aa)?;
    let mut rb = b.runtime();
    let ab = rb.ready()?;
    let repo_b = ArtifactRepository::new(&ab)?;
    let source = repo_a.load_template(template_id())?;
    assert_eq!(
        source.artifact(),
        repo_b.load_template(template_id())?.artifact()
    );
    let before = inventory(&b.root);
    let plan = CanonicalWritePlan::new().replace_template(&template(), source.source())?;
    let error = with_plan(plan, &repo_b, |plan, permit| {
        plan.prepare(&repo_b, permit).unwrap_err()
    });
    checked_error(&error, ArtifactWriteCategory::SourceMismatch);
    assert_eq!(inventory(&b.root), before);
    Ok(())
}

#[test]
fn stale_same_length_revision_and_lexical_only_sources_fail_before_allocation() -> TestResult {
    for lexical in [false, true] {
        let fixture = Fixture::new();
        let old = raw(&document_value());
        fixture.write(ArtifactSourceId::Document(document_id()), &old);
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let loaded = repo.load_document(document_id())?;
        let changed = String::from_utf8(old.clone())?
            .replacen(
                if lexical { "1E100" } else { "G4_PRIVATE" },
                if lexical { "1e100" } else { "G4_private" },
                1,
            )
            .into_bytes();
        assert_eq!(old.len(), changed.len());
        assert_ne!(Sha256::digest(&old), Sha256::digest(&changed));
        let decoded = artifact::decode_document(&changed)?;
        assert_eq!(
            decoded.template_revision(),
            loaded.artifact().template_revision()
        );
        if lexical {
            assert_eq!(&decoded, loaded.artifact());
        } else {
            assert_ne!(&decoded, loaded.artifact());
        }
        fixture.write(ArtifactSourceId::Document(document_id()), &changed);
        let before = inventory(&fixture.root);
        let plan = CanonicalWritePlan::new().replace_document(&document(), loaded.source())?;
        let error = with_plan(plan, &repo, |plan, permit| {
            plan.prepare(&repo, permit).unwrap_err()
        });
        checked_error(&error, ArtifactWriteCategory::SourceMismatch);
        assert_eq!(
            error.diagnostic().prepare_stage,
            Some(PrepareStage::ValidateSchemaTransition)
        );
        assert_eq!(error.diagnostic().transaction_id, None);
        assert_eq!(inventory(&fixture.root), before);
    }
    Ok(())
}

#[test]
fn replacement_disappearance_never_becomes_creation() -> TestResult {
    let fixture = Fixture::new();
    fixture.write(
        ArtifactSourceId::Template(template_id()),
        &raw(&template_value()),
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let loaded = repo.load_template(template_id())?;
    fs::remove_file(fixture.path(ArtifactSourceId::Template(template_id())))?;
    let before = inventory(&fixture.root);
    let plan = CanonicalWritePlan::new().replace_template(&template(), loaded.source())?;
    let error = with_plan(plan, &repo, |plan, permit| {
        plan.prepare(&repo, permit).unwrap_err()
    });
    checked_error(&error, ArtifactWriteCategory::SourceMissing);
    assert_eq!(inventory(&fixture.root), before);
    Ok(())
}

#[test]
fn existing_active_deleted_corrupt_and_future_create_targets_are_not_upserts() -> TestResult {
    for variant in ["active", "deleted", "corrupt", "future"] {
        let fixture = Fixture::new();
        let mut value = template_value();
        if variant == "deleted" {
            value["lifecycle"] = json!("deleted");
        }
        if variant == "future" {
            value["schemaVersion"] = json!(2);
        }
        let bytes = if variant == "corrupt" {
            b"{broken".to_vec()
        } else {
            raw(&value)
        };
        if matches!(variant, "active" | "deleted") {
            artifact::decode_template(&bytes)?;
        }
        fixture.write(ArtifactSourceId::Template(template_id()), &bytes);
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let before = inventory(&fixture.root);
        let plan = CanonicalWritePlan::new().create_template(&template())?;
        let error = with_plan(plan, &repo, |plan, permit| {
            plan.prepare(&repo, permit).unwrap_err()
        });
        checked_error(&error, ArtifactWriteCategory::TargetExists);
        assert_eq!(inventory(&fixture.root), before);
    }
    Ok(())
}

#[test]
fn newly_corrupt_future_and_zero_schema_replacements_fail_codec_admission() -> TestResult {
    for replacement in [
        b"{broken".to_vec(),
        raw(&{
            let mut v = template_value();
            v["schemaVersion"] = json!(8);
            v
        }),
        raw(&{
            let mut v = template_value();
            v["schemaVersion"] = json!(0);
            v
        }),
    ] {
        let fixture = Fixture::new();
        fixture.write(
            ArtifactSourceId::Template(template_id()),
            &raw(&template_value()),
        );
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let loaded = repo.load_template(template_id())?;
        fixture.write(ArtifactSourceId::Template(template_id()), &replacement);
        assert!(repo.load_template(template_id()).is_err());
        let before = inventory(&fixture.root);
        let plan = CanonicalWritePlan::new().replace_template(&template(), loaded.source())?;
        let error = with_plan(plan, &repo, |plan, permit| {
            plan.prepare(&repo, permit).unwrap_err()
        });
        checked_error(&error, ArtifactWriteCategory::CodecRejected);
        assert!(error.diagnostic().codec.is_some());
        assert_eq!(inventory(&fixture.root), before);
    }
    Ok(())
}

struct OriginalRace<'a> {
    path: &'a Path,
    replacement: Option<&'a [u8]>,
    observed: Cell<usize>,
    allocated: Cell<usize>,
}
impl PrepareHooks for OriginalRace<'_> {
    fn check(&self, point: PrepareFailPoint, operation: Option<u32>) -> io::Result<()> {
        if point == PrepareFailPoint::TransactionDirectoryAllocated {
            self.allocated.set(self.allocated.get() + 1);
        }
        if point == PrepareFailPoint::BeforeOriginalRead && operation == Some(0) {
            self.observed.set(self.observed.get() + 1);
            match self.replacement {
                Some(bytes) => fs::write(self.path, bytes)?,
                None => fs::remove_file(self.path)?,
            }
        }
        Ok(())
    }
}
#[test]
fn second_original_read_rejects_changed_deleted_and_newly_appearing_sources() -> TestResult {
    for case in ["replace-change", "replace-delete", "create-appears"] {
        let fixture = Fixture::new();
        let old = raw(&template_value());
        let path = fixture.path(ArtifactSourceId::Template(template_id()));
        if case != "create-appears" {
            fs::write(&path, &old)?;
        }
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let plan = if case == "create-appears" {
            CanonicalWritePlan::new().create_template(&template())?
        } else {
            CanonicalWritePlan::new()
                .replace_template(&template(), repo.load_template(template_id())?.source())?
        };
        let changed = String::from_utf8(old.clone())?
            .replacen("1E100", "1e100", 1)
            .into_bytes();
        assert_eq!(changed.len(), old.len());
        assert_eq!(artifact::decode_template(&changed)?, template());
        let hook = OriginalRace {
            path: &path,
            replacement: if case == "replace-delete" {
                None
            } else {
                Some(&changed)
            },
            observed: Cell::new(0),
            allocated: Cell::new(0),
        };
        let error = with_plan(plan, &repo, |plan, permit| {
            prepare_canonical_with_hooks(plan, &repo, permit, &hook).unwrap_err()
        });
        checked_error(
            &error,
            match case {
                "replace-delete" => ArtifactWriteCategory::SourceMissing,
                "create-appears" => ArtifactWriteCategory::TargetExists,
                _ => ArtifactWriteCategory::SourceMismatch,
            },
        );
        assert_eq!(hook.allocated.get(), 1);
        assert_eq!(hook.observed.get(), 1);
        assert_eq!(
            error.diagnostic().prepare_stage,
            Some(PrepareStage::ReadOriginal)
        );
        assert!(error.diagnostic().transaction_id.is_some());
        assert!(error.diagnostic().cleanup.is_none());
        assert_eq!(
            fs::read_dir(repo.write_project().transactions_root())?.count(),
            0
        );
        if case == "replace-delete" {
            assert!(!path.exists());
        } else {
            assert_eq!(fs::read(&path)?, changed);
        }
    }
    Ok(())
}

#[test]
fn namespace_absence_is_explicit_and_is_not_automatically_created() -> TestResult {
    let fixture = Fixture::new();
    fs::remove_dir(fixture.root.join("templates"))?;
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    assert!(repo.scan_templates()?.records().is_empty());
    let before = inventory(&fixture.root);
    let plan = CanonicalWritePlan::new().create_template(&template())?;
    let error = with_plan(plan, &repo, |plan, permit| {
        plan.prepare(&repo, permit).unwrap_err()
    });
    checked_error(&error, ArtifactWriteCategory::NamespaceUnavailable);
    assert_eq!(inventory(&fixture.root), before);
    Ok(())
}

#[test]
fn empty_wrong_project_missing_and_extra_permits_allocate_nothing() -> TestResult {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let t = ArtifactSourceId::Template(template_id()).path()?;
    let d = ArtifactSourceId::Document(document_id()).path()?;
    for case in ["empty", "wrong-project", "missing", "extra"] {
        let plan = if case == "empty" {
            CanonicalWritePlan::new()
        } else {
            CanonicalWritePlan::new()
                .create_template(&template())?
                .create_document(&document())?
        };
        let targets = match case {
            "missing" | "empty" => vec![t.clone()],
            "extra" => vec![
                t.clone(),
                d.clone(),
                ArtifactSourceId::Template(OTHER.parse()?).path()?,
            ],
            _ => vec![t.clone(), d.clone()],
        };
        let fingerprint = if case == "wrong-project" {
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        } else {
            repo.write_project().fingerprint()
        };
        let before = inventory(&fixture.root);
        let error = with_permit(fingerprint, targets, |permit| {
            plan.prepare(&repo, permit).unwrap_err()
        });
        checked_error(
            &error,
            if case == "empty" {
                ArtifactWriteCategory::EmptyPlan
            } else {
                ArtifactWriteCategory::PermitRejected
            },
        );
        assert_eq!(inventory(&fixture.root), before);
    }
    Ok(())
}

#[test]
fn read_only_template_is_not_a_document_write_target() -> TestResult {
    let fixture = Fixture::new();
    fixture.write(
        ArtifactSourceId::Template(template_id()),
        &raw(&template_value()),
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let source = repo.load_template(template_id())?;
    let plan = CanonicalWritePlan::new().create_document(&document())?;
    assert_eq!(
        plan.targets().cloned().collect::<Vec<_>>(),
        vec![ArtifactSourceId::Document(document_id()).path()?]
    );
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        assert_eq!(
            plan.prepare(&repo, permit)?.commit()?.result_state(),
            CommitResultState::Committed
        );
        Ok(())
    })?;
    assert!(repo.reread_matches(source.source())?);
    Ok(())
}

#[test]
fn storage_admission_uses_real_targets_and_preserves_query_and_space_failures() -> TestResult {
    for response in [
        TestStorageResponse::Available {
            available_bytes: 0,
            allocation_unit_bytes: 4096,
        },
        TestStorageResponse::QueryFailure,
        TestStorageResponse::CrossFilesystem,
        TestStorageResponse::Available {
            available_bytes: u64::MAX,
            allocation_unit_bytes: u64::MAX,
        },
    ] {
        let fixture = Fixture::new();
        fixture.write(
            ArtifactSourceId::Template(template_id()),
            &raw(&template_value()),
        );
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let loaded = repo.load_template(template_id())?;
        let before = inventory(&fixture.root);
        let plan = CanonicalWritePlan::new()
            .replace_template(&template(), loaded.source())?
            .create_document(&document())?;
        let (error, records) = with_storage_response(response, || {
            with_plan(plan, &repo, |plan, permit| {
                plan.prepare(&repo, permit).unwrap_err()
            })
        });
        checked_error(&error, ArtifactWriteCategory::StorageRejected);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].target_paths.len(), 2);
        let mut expected = vec![
            repo.write_project()
                .canonical_root()
                .join(ArtifactSourceId::Template(template_id()).path()?.as_str()),
            repo.write_project()
                .canonical_root()
                .join(ArtifactSourceId::Document(document_id()).path()?.as_str()),
        ];
        expected.sort();
        assert_eq!(records[0].target_paths, expected);
        match response {
            TestStorageResponse::QueryFailure => assert_eq!(
                error.diagnostic().storage_query,
                Some(StorageQueryErrorKind::QueryFailed)
            ),
            TestStorageResponse::CrossFilesystem => assert_eq!(
                error.diagnostic().storage_query,
                Some(StorageQueryErrorKind::CrossFilesystem)
            ),
            TestStorageResponse::Available {
                available_bytes: 0, ..
            } => {
                assert!(error.diagnostic().storage.unwrap().required_peak_bytes() > 8 * 1024 * 1024)
            }
            _ => {}
        }
        assert_eq!(inventory(&fixture.root), before);
    }
    Ok(())
}

struct FailureHook {
    point: PrepareFailPoint,
    cleanup: bool,
    hits: Cell<usize>,
}
impl PrepareHooks for FailureHook {
    fn check(&self, point: PrepareFailPoint, _: Option<u32>) -> io::Result<()> {
        if point == self.point || (self.cleanup && point == PrepareFailPoint::Cleanup) {
            self.hits.set(self.hits.get() + 1);
            return Err(io::Error::other(CANARY));
        }
        Ok(())
    }
}
#[test]
fn staging_backup_manifest_and_state_faults_preserve_targets_and_cleanup() -> TestResult {
    for point in [
        PrepareFailPoint::StagedWrite,
        PrepareFailPoint::StagedSync,
        PrepareFailPoint::BackupWrite,
        PrepareFailPoint::ManifestCreate,
        PrepareFailPoint::ManifestVerify,
        PrepareFailPoint::PreparedState,
    ] {
        let fixture = Fixture::new();
        let old = raw(&template_value());
        fixture.write(ArtifactSourceId::Template(template_id()), &old);
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let plan = CanonicalWritePlan::new()
            .replace_template(&template(), repo.load_template(template_id())?.source())?;
        let hook = FailureHook {
            point,
            cleanup: false,
            hits: Cell::new(0),
        };
        let error = with_plan(plan, &repo, |plan, permit| {
            prepare_canonical_with_hooks(plan, &repo, permit, &hook).unwrap_err()
        });
        checked_error(&error, ArtifactWriteCategory::PrepareFailed);
        assert_eq!(hook.hits.get(), 1);
        assert!(error.diagnostic().transaction_id.is_some());
        assert!(error.diagnostic().cleanup.is_none());
        assert_eq!(error.diagnostic().io.unwrap().kind, io::ErrorKind::Other);
        assert_eq!(
            fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
            old
        );
        assert_eq!(
            fs::read_dir(repo.write_project().transactions_root())?.count(),
            0
        );
    }
    Ok(())
}

#[test]
fn initial_and_cleanup_failures_keep_transaction_identity_for_explicit_recovery() -> TestResult {
    let fixture = Fixture::new();
    let old = raw(&template_value());
    fixture.write(ArtifactSourceId::Template(template_id()), &old);
    let mut runtime = fixture.runtime();
    {
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let plan = CanonicalWritePlan::new()
            .replace_template(&template(), repo.load_template(template_id())?.source())?;
        let hook = FailureHook {
            point: PrepareFailPoint::StagedSync,
            cleanup: true,
            hits: Cell::new(0),
        };
        let error = with_plan(plan, &repo, |plan, permit| {
            prepare_canonical_with_hooks(plan, &repo, permit, &hook).unwrap_err()
        });
        checked_error(&error, ArtifactWriteCategory::PrepareFailed);
        assert_eq!(hook.hits.get(), 2);
        assert_eq!(
            error.diagnostic().prepare_stage,
            Some(PrepareStage::WriteStaged)
        );
        assert_eq!(
            error.diagnostic().cleanup.unwrap().kind,
            io::ErrorKind::Other
        );
        assert!(repo
            .write_project()
            .transactions_root()
            .join(error.diagnostic().transaction_id.as_ref().unwrap().as_str())
            .exists());
        assert_eq!(
            fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
            old
        );
    }
    let summary = runtime.recover()?;
    assert_eq!(summary.preparing_orphan_cleanups, 1);
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
        old
    );
    Ok(())
}

#[test]
fn dropping_prepared_preserves_journal_until_existing_m1_recovery() -> TestResult {
    let fixture = Fixture::new();
    let old = raw(&template_value());
    fixture.write(ArtifactSourceId::Template(template_id()), &old);
    let mut runtime = fixture.runtime();
    {
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let plan = CanonicalWritePlan::new()
            .replace_template(&template(), repo.load_template(template_id())?.source())?
            .create_document(&document())?;
        let directory = with_plan(
            plan,
            &repo,
            |plan, permit| -> Result<PathBuf, ArtifactWriteError> {
                let prepared = plan.prepare(&repo, permit)?;
                let directory = prepared
                    .inner_for_test()
                    .transaction_directory()
                    .to_path_buf();
                drop(prepared);
                Ok(directory)
            },
        )?;
        assert!(directory.join("manifest.json").is_file());
        assert!(directory.join("state.json").is_file());
    }
    assert_eq!(runtime.recover()?.rolled_back_transactions, 1);
    assert_eq!(
        fs::read(fixture.path(ArtifactSourceId::Template(template_id())))?,
        old
    );
    assert!(!fixture
        .path(ArtifactSourceId::Document(document_id()))
        .exists());
    Ok(())
}

#[test]
fn second_encode_fault_consumes_partial_plan_without_touching_filesystem() -> TestResult {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let _repo = ArtifactRepository::new(&ready)?;
    let before = inventory(&fixture.root);
    let t = template();
    let d = document();
    let tc = t.clone();
    let dc = d.clone();
    let plan = CanonicalWritePlan::new().create_template(&t)?;
    // sealed aggregate의 불변식을 깨는 production mutator 대신 닫힌 encode 호출에만 fault를 넣는다.
    let fault = artifact::decode_document(b"{invalid").unwrap_err();
    let error = plan
        .add(
            ArtifactSourceId::Document(d.document_id()),
            ArtifactIntent::Create,
            || Err(fault),
        )
        .unwrap_err();
    checked_error(&error, ArtifactWriteCategory::CodecRejected);
    assert_eq!(error.diagnostic().stage, ArtifactWriteStage::Encode);
    assert_eq!(t, tc);
    assert_eq!(d, dc);
    assert_eq!(inventory(&fixture.root), before);
    Ok(())
}

#[test]
fn second_source_admission_failure_never_prepares_first_target() -> TestResult {
    let fixture = Fixture::new();
    fixture.write(
        ArtifactSourceId::Template(template_id()),
        &raw(&template_value()),
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let before = inventory(&fixture.root);
    let plan = CanonicalWritePlan::new()
        .create_document(&document())?
        .create_template(&template())?;
    let error = with_plan(plan, &repo, |plan, permit| {
        plan.prepare(&repo, permit).unwrap_err()
    });
    checked_error(&error, ArtifactWriteCategory::TargetExists);
    assert_eq!(inventory(&fixture.root), before);
    Ok(())
}

#[test]
fn canonical_schema_defense_does_not_treat_replacement_as_migration() -> TestResult {
    let fixture = Fixture::new();
    fixture.write(
        ArtifactSourceId::Template(template_id()),
        &raw(&template_value()),
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let before = inventory(&fixture.root);
    let mut plan = CanonicalWritePlan::new()
        .replace_template(&template(), repo.load_template(template_id())?.source())?;
    // v1 public candidate는 schema를 바꿀 수 없다. M1의 방어 분기만 private test에서 주입한다.
    plan.writes[0].bytes = String::from_utf8(plan.writes[0].bytes.clone())?
        .replace("\"schemaVersion\": 1", "\"schemaVersion\": 2")
        .into_bytes();
    assert!(String::from_utf8(plan.writes[0].bytes.clone())?.contains("\"schemaVersion\": 2"));
    let error = with_plan(plan, &repo, |plan, permit| {
        plan.prepare(&repo, permit).unwrap_err()
    });
    checked_error(&error, ArtifactWriteCategory::SchemaMismatch);
    assert_eq!(inventory(&fixture.root), before);
    Ok(())
}

#[test]
fn m36_format_transition_preserves_exact_history_and_explicit_restore() -> TestResult {
    for id in [
        ArtifactSourceId::Template(template_id()),
        ArtifactSourceId::Document(document_id()),
    ] {
        let fixture = Fixture::new();
        let value = match id {
            ArtifactSourceId::Template(_) => template_value(),
            _ => document_value(),
        };
        let old = String::from_utf8(raw(&value))?
            .replace("\"future\":null", &format!("\"future\":{LEXEMES}"))
            .into_bytes();
        fixture.write(id, &old);
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let (schema, hash, history) = super::super::format::inspect(&repo, id)?;
        assert_eq!(schema, 1);
        assert!(history.is_empty());
        let plan = super::super::format::build(&repo, id, &hash, None)?;
        with_plan(plan, &repo, |plan, permit| -> TestResult {
            assert_eq!(
                plan.prepare(&repo, permit)?.commit()?.result_state(),
                CommitResultState::Committed
            );
            Ok(())
        })?;
        let upgraded = fs::read(fixture.path(id))?;
        assert!(String::from_utf8(upgraded.clone())?.contains("1E100"));
        assert!(String::from_utf8(upgraded.clone())?.contains("0.12345678901234567890123456789"));
        let (_, new_hash, history) = super::super::format::inspect(&repo, id)?;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].digest, hash);
        assert!(super::super::format::build(&repo, id, &hash, Some(&hash)).is_err());
        let plan = super::super::format::build(&repo, id, &new_hash, Some(&hash))?;
        with_plan(plan, &repo, |plan, permit| -> TestResult {
            assert_eq!(
                plan.prepare(&repo, permit)?.commit()?.result_state(),
                CommitResultState::Committed
            );
            Ok(())
        })?;
        assert_eq!(fs::read(fixture.path(id))?, old);
        assert_eq!(super::super::format::history(&repo, id)?.len(), 2);
    }
    Ok(())
}
#[test]
fn m36_format_prepared_and_pending_recovery_keep_original_and_archive() -> TestResult {
    for committed in [false, true] {
        let fixture = Fixture::new();
        let id = ArtifactSourceId::Document(document_id());
        let old = raw(&document_value());
        fixture.write(id, &old);
        let mut runtime = fixture.runtime();
        {
            let ready = runtime.ready()?;
            let repo = ArtifactRepository::new(&ready)?;
            let (_, hash, _) = super::super::format::inspect(&repo, id)?;
            let plan = super::super::format::build(&repo, id, &hash, None)?;
            with_plan(plan, &repo, |plan, permit| -> TestResult {
                let prepared = plan.prepare(&repo, permit)?;
                if committed {
                    prepared.commit()?;
                } // 미완료 journal은 기존 runtime recovery가 회수한다.
                Ok(())
            })?;
            assert_eq!(super::super::format::history(&repo, id)?.len(), 1);
        }
        if !committed {
            assert_eq!(runtime.recover()?.rolled_back_transactions, 1);
            assert_eq!(fs::read(fixture.path(id))?, old);
        }
    }
    Ok(())
}

#[test]
fn m36_format_disk_admission_failures_preserve_exact_original_and_history() -> TestResult {
    for response in [
        TestStorageResponse::Available {
            available_bytes: 0,
            allocation_unit_bytes: 4096,
        },
        TestStorageResponse::QueryFailure,
    ] {
        let fixture = Fixture::new();
        let id = ArtifactSourceId::Template(template_id());
        let original = raw(&template_value());
        fixture.write(id, &original);
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let (_, hash, _) = super::super::format::inspect(&repo, id)?;
        let plan = super::super::format::build(&repo, id, &hash, None)?;
        let (error, records) = with_storage_response(response, || {
            with_plan(plan, &repo, |plan, permit| {
                plan.prepare(&repo, permit).unwrap_err()
            })
        });
        checked_error(&error, ArtifactWriteCategory::StorageRejected);
        assert_eq!(records.len(), 1);
        assert_eq!(fs::read(fixture.path(id))?, original);
        let (schema, unchanged_hash, history) = super::super::format::inspect(&repo, id)?;
        assert_eq!(schema, 1);
        assert_eq!(unchanged_hash, hash);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].digest, hash);
        let pending = match fs::read_dir(repo.write_project().transactions_root()) {
            Ok(entries) => entries.count(),
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e.into()),
        };
        assert_eq!(pending, 0);
    }
    Ok(())
}
