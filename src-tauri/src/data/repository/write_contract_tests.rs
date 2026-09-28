//! 실제 업무 API가 반환한 owner를 끝까지 유지한 채 G4에 aggregate만 빌려준다.
use super::*;
use crate::data::{
    artifact::{
        DocumentEdit, DocumentEditSet, DocumentReconciliationWarningCategory,
        DocumentSaveOutcomeKind,
    },
    json::parse_strict_lossless_json_object,
    transaction::test_support::{with_commit_failures, CommitTestPoint},
};
#[path = "write_path_contract_tests.rs"]
mod paths;
#[path = "write_projection_tests.rs"]
mod projection;

const LATER: &str = "2026-09-08T02:03:04.005Z";
const OPTION: &str = "44444444-4444-4444-8444-444444444444";

fn warning_fixture() -> (TemplateArtifact, DocumentArtifact, Vec<u8>) {
    let mut t = template_value();
    t["fieldOrder"] = json!([FIELD]);
    t["fields"][FIELD] = json!({"label":CANARY,"kind":"singleChoice","lifecycle":"active",
        "required":false,"introducedRevision":1,"defaultValue":{"kind":"unset"},
        "initialDefaultValue":{"kind":"unset"},"presentation":{},
        "configuration":{"kind":"singleChoice","optionOrder":[],
            "options":{(OPTION):{"label":CANARY,"lifecycle":"archived"}}}});
    let mut d = document_value();
    d["fieldValues"][FIELD] = json!({"kind":"singleChoice","optionId":OPTION,"future":null});
    d["orphanedFieldDefinitions"][FIELD] = json!({"kind":"singleChoice","label":CANARY,"future":null,
        "options":{(OPTION):{"label":CANARY,"future":null}}});
    d["fieldValues"][OTHER] = json!({"kind":"text","value":CANARY,"future":null});
    d["orphanedFieldDefinitions"][OTHER] =
        json!({"kind":"singleLineText","label":CANARY,"options":{},"future":null});
    let mut original = raw(&d);
    original.extend_from_slice(b" \r\n  ");
    (
        artifact::decode_template(&raw(&t)).unwrap(),
        artifact::decode_document(&original).unwrap(),
        original,
    )
}

fn assert_owned_metadata(bytes: &[u8]) {
    let tree = parse_strict_lossless_json_object(bytes).expect("valid document");
    let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).expect("fixed metadata");
    for path in [
        vec!["future"],
        vec!["fieldValues", FIELD, "future"],
        vec!["fieldValues", OTHER, "future"],
        vec!["orphanedFieldDefinitions", OTHER, "future"],
        vec!["orphanedFieldDefinitions", FIELD, "future"],
        vec![
            "orphanedFieldDefinitions",
            FIELD,
            "options",
            OPTION,
            "future",
        ],
    ] {
        let actual = tree.object_path(&path).expect("required metadata owner");
        assert!(
            actual == &expected,
            "metadata owner or number lexemes changed"
        );
    }
    let label = tree
        .object_path(&["orphanedFieldDefinitions", OTHER, "label"])
        .expect("orphan label");
    let expected_label =
        parse_strict_lossless_json_object(&serde_json::to_vec(&json!({"label":CANARY})).unwrap())
            .unwrap();
    assert!(
        Some(label) == expected_label.object_path(&["label"]),
        "orphan label changed"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Boundary {
    Commit,
    Encode,
    Admission,
    Prepare,
    CommitFailure,
}

#[test]
fn g1_nonempty_warning_owner_survives_success_and_four_failure_boundaries() -> TestResult {
    let (template, source, original_raw) = warning_fixture();
    let original_template = artifact::encode_template(&template)?;
    let original_source = artifact::encode_document(&source)?;
    let edits = DocumentEditSet::new(vec![DocumentEdit::Rename("edited name".into())]);
    let original_edits = edits.clone();
    let outcome =
        artifact::prepare_document_save(&template, template.revision(), &source, &edits, LATER)?;
    let candidate = artifact::encode_document(outcome.document())?;
    let outcome_owner = &outcome as *const _;
    let warning_owner = outcome.warnings().as_slice().as_ptr();
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
    assert_eq!(outcome.document().updated_at_utc(), LATER);
    assert_eq!(outcome.warnings().len(), 1);
    assert_owned_metadata(&original_source);
    assert_owned_metadata(&candidate);
    for boundary in [
        Boundary::Commit,
        Boundary::Encode,
        Boundary::Admission,
        Boundary::Prepare,
        Boundary::CommitFailure,
    ] {
        let fixture = Fixture::new();
        fixture.write(ArtifactSourceId::Document(document_id()), &original_raw);
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let loaded = repo.load_document(document_id())?;
        let before = inventory(&fixture.root);
        if boundary == Boundary::Encode {
            // sealed aggregate에서는 만들 수 없는 실패이므로 기존 private encode 호출만 주입한다.
            let codec = artifact::decode_document(b"{invalid").unwrap_err();
            let error = CanonicalWritePlan::new()
                .add(
                    ArtifactSourceId::Document(document_id()),
                    ArtifactIntent::Replace(loaded.source().clone()),
                    || Err(codec),
                )
                .unwrap_err();
            checked_error(&error, ArtifactWriteCategory::CodecRejected);
            assert!(
                inventory(&fixture.root) == before,
                "encode touched filesystem"
            );
        } else {
            let plan =
                CanonicalWritePlan::new().replace_document(outcome.document(), loaded.source())?;
            with_plan(plan, &repo, |plan, permit| -> TestResult {
                match boundary {
                    Boundary::Admission => {
                        let (error, records) = with_storage_response(
                            TestStorageResponse::Available {
                                available_bytes: 0,
                                allocation_unit_bytes: 4096,
                            },
                            || plan.prepare(&repo, permit).unwrap_err(),
                        );
                        checked_error(&error, ArtifactWriteCategory::StorageRejected);
                        assert_eq!(records.len(), 1);
                        assert!(
                            inventory(&fixture.root) == before,
                            "admission touched filesystem"
                        );
                    }
                    Boundary::Prepare => {
                        let hook = FailureHook {
                            point: PrepareFailPoint::StagedWrite,
                            cleanup: false,
                            hits: Cell::new(0),
                        };
                        let error =
                            prepare_canonical_with_hooks(plan, &repo, permit, &hook).unwrap_err();
                        checked_error(&error, ArtifactWriteCategory::PrepareFailed);
                        assert_eq!(hook.hits.get(), 1);
                    }
                    Boundary::Commit | Boundary::CommitFailure => {
                        let prepared = plan.prepare(&repo, permit)?;
                        let op = &prepared.inner_for_test().manifest().operations[0];
                        let backup = op.backup_path.as_ref().expect("replace backup");
                        assert!(
                            fs::read(
                                prepared
                                    .inner_for_test()
                                    .transaction_directory()
                                    .join(backup)
                            )? == original_raw,
                            "backup changed source bytes"
                        );
                        assert!(
                            fs::read(
                                prepared
                                    .inner_for_test()
                                    .transaction_directory()
                                    .join(&op.staged_path)
                            )? == candidate,
                            "staged candidate differs"
                        );
                        if boundary == Boundary::Commit {
                            assert_eq!(
                                prepared.commit()?.result_state(),
                                CommitResultState::Committed
                            );
                        } else {
                            let error = with_commit_failures(
                                Some(CommitTestPoint::ManifestRevalidation),
                                None,
                                || prepared.commit(),
                            )
                            .unwrap_err();
                            assert_eq!(error.result_state(), CommitResultState::NotApplied);
                            no_leak(&error);
                            assert!(error.source().is_none());
                        }
                    }
                    Boundary::Encode => unreachable!("encode has a separate private seam"),
                }
                Ok(())
            })?;
        }
        let actual = fs::read(fixture.path(ArtifactSourceId::Document(document_id())))?;
        assert!(
            actual
                == if boundary == Boundary::Commit {
                    candidate.clone()
                } else {
                    original_raw.clone()
                },
            "target differs from boundary contract"
        );
        assert!(artifact::decode_document(&actual).is_ok());
        assert!(std::ptr::eq(&outcome, outcome_owner), "outcome owner moved");
        assert!(
            std::ptr::eq(outcome.warnings().as_slice().as_ptr(), warning_owner),
            "warning owner replaced"
        );
        assert_eq!(outcome.warnings().len(), 1);
        let warning = &outcome.warnings()[0];
        assert_eq!(
            warning.category(),
            DocumentReconciliationWarningCategory::ArchivedOptionSelected
        );
        assert_eq!(warning.field_id(), FIELD.parse()?);
        assert_eq!(warning.count(), 1);
        assert!(!warning.truncated());
        assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
        assert_eq!(outcome.document().updated_at_utc(), LATER);
        assert_eq!(source.updated_at_utc(), TIME);
        assert!(
            artifact::encode_document(outcome.document())? == candidate,
            "candidate changed"
        );
        assert!(
            artifact::encode_document(&source)? == original_source,
            "source changed"
        );
        assert!(
            artifact::encode_template(&template)? == original_template,
            "template changed"
        );
        assert!(edits == original_edits, "edits changed");
    }
    Ok(())
}

#[test]
fn g1_unchanged_warning_and_raw_whitespace_do_not_advance_timestamp() -> TestResult {
    let (template, source, original_raw) = warning_fixture();
    let edits = DocumentEditSet::new(vec![DocumentEdit::Rename(CANARY.into())]);
    let edits_before = edits.clone();
    let outcome =
        artifact::prepare_document_save(&template, template.revision(), &source, &edits, LATER)?;
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert_eq!(outcome.warnings().len(), 1);
    let warning_owner = outcome.warnings().as_slice().as_ptr();
    let bytes = artifact::encode_document(outcome.document())?;
    assert!(
        bytes != original_raw,
        "fixture must have different raw whitespace"
    );
    let fixture = Fixture::new();
    fixture.write(ArtifactSourceId::Document(document_id()), &original_raw);
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let loaded = repo.load_document(document_id())?;
    let rejected_plan =
        CanonicalWritePlan::new().replace_document(outcome.document(), loaded.source())?;
    let (error, _) = with_storage_response(TestStorageResponse::QueryFailure, || {
        with_plan(rejected_plan, &repo, |plan, permit| {
            plan.prepare(&repo, permit).unwrap_err()
        })
    });
    checked_error(&error, ArtifactWriteCategory::StorageRejected);
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert_eq!(outcome.document().updated_at_utc(), TIME);
    assert_eq!(outcome.warnings().len(), 1);
    assert!(std::ptr::eq(
        outcome.warnings().as_slice().as_ptr(),
        warning_owner
    ));
    assert!(
        artifact::encode_document(outcome.document())? == bytes,
        "unchanged candidate changed after failure"
    );
    assert!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))? == original_raw,
        "unchanged failure touched source"
    );
    let plan = CanonicalWritePlan::new().replace_document(outcome.document(), loaded.source())?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        assert_eq!(
            plan.prepare(&repo, permit)?.commit()?.result_state(),
            CommitResultState::Committed
        );
        Ok(())
    })?;
    assert!(
        fs::read(fixture.path(ArtifactSourceId::Document(document_id())))? == bytes,
        "canonical target differs"
    );
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert_eq!(outcome.document().updated_at_utc(), TIME);
    assert_eq!(source.updated_at_utc(), TIME);
    assert_eq!(outcome.warnings().len(), 1);
    assert!(std::ptr::eq(
        outcome.warnings().as_slice().as_ptr(),
        warning_owner
    ));
    assert!(
        artifact::encode_document(&source)? == bytes,
        "unchanged aggregate changed"
    );
    assert!(edits == edits_before, "unchanged edits changed");
    assert_owned_metadata(&bytes);
    Ok(())
}

#[test]
fn p2_creation_and_duplication_owners_survive_adapter_boundaries() -> TestResult {
    let created = artifact::create_template(CANARY.into(), Some(CANARY.into()), TIME.into())?;
    let duplicate_source = template();
    let source_before = artifact::encode_template(&duplicate_source)?;
    let duplicated = artifact::duplicate_template(&duplicate_source, LATER.into())?;
    assert_ne!(duplicated.template_id(), duplicate_source.template_id());
    assert_eq!(duplicated.revision(), TemplateRevision::INITIAL);
    for candidate in [&created, &duplicated] {
        let bytes = artifact::encode_template(candidate)?;
        let id = ArtifactSourceId::Template(candidate.template_id());
        for boundary in [
            Boundary::Commit,
            Boundary::Encode,
            Boundary::Admission,
            Boundary::Prepare,
            Boundary::CommitFailure,
        ] {
            let fixture = Fixture::new();
            let mut runtime = fixture.runtime();
            let ready = runtime.ready()?;
            let repo = ArtifactRepository::new(&ready)?;
            if boundary == Boundary::Encode {
                let fault = artifact::decode_template(b"{invalid").unwrap_err();
                let error = CanonicalWritePlan::new()
                    .add(id, ArtifactIntent::Create, || Err(fault))
                    .unwrap_err();
                checked_error(&error, ArtifactWriteCategory::CodecRejected);
            } else {
                let plan = CanonicalWritePlan::new().create_template(candidate)?;
                with_plan(plan, &repo, |plan, permit| -> TestResult {
                    match boundary {
                        Boundary::Admission => {
                            let (error, _) =
                                with_storage_response(TestStorageResponse::QueryFailure, || {
                                    plan.prepare(&repo, permit).unwrap_err()
                                });
                            checked_error(&error, ArtifactWriteCategory::StorageRejected);
                        }
                        Boundary::Prepare => {
                            let hook = FailureHook {
                                point: PrepareFailPoint::PreparedState,
                                cleanup: false,
                                hits: Cell::new(0),
                            };
                            let error = prepare_canonical_with_hooks(plan, &repo, permit, &hook)
                                .unwrap_err();
                            checked_error(&error, ArtifactWriteCategory::PrepareFailed);
                            assert_eq!(hook.hits.get(), 1);
                        }
                        Boundary::Commit => assert_eq!(
                            plan.prepare(&repo, permit)?.commit()?.result_state(),
                            CommitResultState::Committed
                        ),
                        Boundary::CommitFailure => {
                            let prepared = plan.prepare(&repo, permit)?;
                            let error = with_commit_failures(
                                Some(CommitTestPoint::ManifestRevalidation),
                                None,
                                || prepared.commit(),
                            )
                            .unwrap_err();
                            assert_eq!(error.result_state(), CommitResultState::NotApplied);
                        }
                        Boundary::Encode => unreachable!("separate encode seam"),
                    }
                    Ok(())
                })?;
            }
            if boundary == Boundary::Commit {
                let target = fs::read(fixture.path(id))?;
                assert!(target == bytes, "P2 target differs");
                assert!(
                    artifact::decode_template(&target)? == *candidate,
                    "P2 redecode differs"
                );
            } else {
                assert!(!fixture.path(id).exists());
            }
            assert!(
                artifact::encode_template(candidate)? == bytes,
                "P2 owner changed"
            );
            assert!(
                artifact::encode_template(&duplicate_source)? == source_before,
                "P2 original changed"
            );
        }
    }
    assert_eq!(created.updated_at_utc(), TIME);
    assert_eq!(duplicated.updated_at_utc(), LATER);
    Ok(())
}
