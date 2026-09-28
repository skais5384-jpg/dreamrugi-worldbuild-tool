use super::*;
use crate::data::artifact::{
    DocumentEdit, DocumentEditSet, DocumentSaveOutcome, DocumentSaveOutcomeKind,
};
use crate::data::json::parse_strict_lossless_json_object;

const FIELD: &str = "33333333-3333-4333-8333-333333333333";
const OPTION: &str = "44444444-4444-4444-8444-444444444444";
const LATER: &str = "2026-09-08T02:03:04.005Z";
const LEXEMES: &str = r#"{"large":1E100,"zero":-0,"fraction":0.12345678901234567890123456789}"#;
fn warning_fixture() -> (Vec<u8>, Vec<u8>) {
    // 기존 G4/G1 archived-option fixture를 같은 codec과 소유 경로로 사용한다.
    let mut template: serde_json::Value =
        serde_json::from_slice(&artifact::encode_template(&template(CANARY)).unwrap()).unwrap();
    template["fieldOrder"] = serde_json::json!([FIELD]);
    template["fields"][FIELD] = serde_json::json!({"label":CANARY,"kind":"singleChoice","lifecycle":"active","required":false,"introducedRevision":1,
        "defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},"presentation":{},"configuration":{"kind":"singleChoice","optionOrder":[],"options":{(OPTION):{"label":CANARY,"lifecycle":"archived"}}}});
    let mut document: serde_json::Value =
        serde_json::from_slice(&artifact::encode_document(&document()).unwrap()).unwrap();
    document["fieldValues"][FIELD] =
        serde_json::json!({"kind":"singleChoice","optionId":OPTION,"future":null});
    document["orphanedFieldDefinitions"][FIELD] = serde_json::json!({"kind":"singleChoice","label":CANARY,"future":null,"options":{(OPTION):{"label":CANARY,"future":null}}});
    (
        serde_json::to_vec(&template).unwrap(),
        serde_json::to_string(&document)
            .unwrap()
            .replace("\"future\":null", &format!("\"future\":{LEXEMES}"))
            .into_bytes(),
    )
}
fn assert_metadata(bytes: &[u8]) {
    let tree = parse_strict_lossless_json_object(bytes).unwrap();
    let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).unwrap();
    for path in [
        vec!["fieldValues", FIELD, "future"],
        vec!["orphanedFieldDefinitions", FIELD, "future"],
        vec![
            "orphanedFieldDefinitions",
            FIELD,
            "options",
            OPTION,
            "future",
        ],
    ] {
        assert!(
            tree.object_path(&path)
                .expect("metadata owner exists before comparison")
                == &expected
        );
    }
}

#[test]
fn real_g1_candidate_warnings_and_number_lexemes_survive_success_and_admission_failure() {
    use crate::data::storage_estimate::test_support::{with_storage_response, TestStorageResponse};
    struct Input {
        edits: DocumentEditSet,
        owner: Option<DocumentSaveOutcome>,
    }
    for denied in [false, true] {
        let fixture = Fixture::new();
        let (template, document) = warning_fixture();
        assert_metadata(&document);
        fs::write(fixture.path(tid()), &template).unwrap();
        fs::write(fixture.path(did()), &document).unwrap();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([did()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut input = Input {
            edits: DocumentEditSet::new(vec![DocumentEdit::Rename("changed name".into())]),
            owner: None,
        };
        let mut owned_address = 0;
        let mut warnings_address = 0;
        let mut candidate_bytes = Vec::new();
        let mut build = |repo: &ArtifactRepository<'_, '_>,
                         input: &mut Input|
         -> Result<_, BuildError<artifact::DocumentSaveError>> {
            let template = repo.load_template(template_id())?;
            let source = repo.load_document(document_id())?;
            let outcome = artifact::prepare_document_save(
                template.artifact(),
                template.artifact().revision(),
                source.artifact(),
                &input.edits,
                LATER,
            )
            .map_err(BuildError::domain)?;
            assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
            assert_eq!(outcome.warnings().len(), 1);
            assert!(format!("{:?}", outcome.warnings()).contains("ArchivedOptionSelected"));
            input.owner = Some(outcome);
            let owner = input.owner.as_ref().unwrap();
            owned_address = owner as *const _ as usize;
            warnings_address = owner.warnings().as_slice().as_ptr() as usize;
            candidate_bytes = artifact::encode_document(owner.document()).unwrap();
            assert_metadata(&candidate_bytes);
            Ok(WriteDecision::Write {
                plan: CanonicalWritePlan::new()
                    .replace_document(owner.document(), source.source())?,
                value: (),
            })
        };
        let mut execute =
            || observe(|| run(&mut runtime, &mut session, &targets, &mut input, &mut build));
        let (result, prepare, commits) = if denied {
            with_storage_response(
                TestStorageResponse::Available {
                    available_bytes: 0,
                    allocation_unit_bytes: 4096,
                },
                execute,
            )
            .0
        } else {
            execute()
        };
        let owner = input.owner.as_ref().unwrap();
        assert_eq!(owner as *const _ as usize, owned_address);
        assert_eq!(
            owner.warnings().as_slice().as_ptr() as usize,
            warnings_address
        );
        assert_eq!(owner.warnings().len(), 1);
        assert_eq!(owner.document().updated_at_utc(), LATER);
        assert_eq!(
            artifact::encode_document(owner.document()).unwrap(),
            candidate_bytes
        );
        assert_metadata(&candidate_bytes);
        assert_eq!(prepare.calls, 1);
        assert_eq!(
            (prepare.allocations, commits),
            if denied { (0, 0) } else { (1, 1) }
        );
        assert_eq!(
            result.diagnostic().disk,
            if denied {
                DiskState::NotApplied
            } else {
                DiskState::Committed
            }
        );
        assert_eq!(
            fs::read(fixture.path(did())).unwrap(),
            if denied { document } else { candidate_bytes }
        );
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn explicit_no_write_preserves_warning_owner_timestamp_and_borrowed_builder() {
    struct Owner {
        warning: String,
    }
    let fixture = Fixture::new();
    fs::write(
        fixture.path(tid()),
        artifact::encode_template(&template(CANARY)).unwrap(),
    )
    .unwrap();
    let mut runtime = fixture.runtime();
    let targets = ExactWriteTargets::new([tid()]).unwrap();
    let mut session = session(&runtime, &targets);
    let before = inventory(&fixture.root);
    let timestamp = fs::metadata(fixture.path(tid()))
        .unwrap()
        .modified()
        .unwrap();
    let mut input = Owner {
        warning: CANARY.to_owned(),
    };
    let owner = input.warning.as_ptr();
    let capture = Owner {
        warning: CANARY.to_owned(),
    };
    let mut build =
        |repo: &ArtifactRepository<'_, '_>, input: &mut Owner| -> Result<_, BuildError<()>> {
            assert_eq!(capture.warning, CANARY);
            let loaded = repo.load_template(template_id())?;
            assert_eq!(loaded.artifact().updated_at_utc(), TIME);
            Ok(WriteDecision::NoWrite(Owner {
                warning: std::mem::take(&mut input.warning),
            }))
        };
    let (result, prepare, commits) =
        observe(|| run(&mut runtime, &mut session, &targets, &mut input, &mut build));
    let BodyOutcome::NoWrite(value) = result.body().unwrap() else {
        panic!("{result:?}")
    };
    assert_eq!(value.warning.as_ptr(), owner);
    assert_eq!(value.warning, CANARY);
    assert_eq!(capture.warning, CANARY);
    assert_eq!((prepare.calls, prepare.allocations, commits), (0, 0, 0));
    assert_eq!(inventory(&fixture.root), before);
    assert_eq!(
        fs::metadata(fixture.path(tid()))
            .unwrap()
            .modified()
            .unwrap(),
        timestamp
    );
    assert!(!format!("{result:?}").contains(CANARY));
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn domain_error_keeps_nonclone_original_owner_without_debug_or_source_leak() {
    struct Secret(String);
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let targets = ExactWriteTargets::new([tid()]).unwrap();
    let mut session = session(&runtime, &targets);
    let mut input = Secret(CANARY.into());
    let owner = input.0.as_ptr();
    let mut build = |_: &ArtifactRepository<'_, '_>,
                     input: &mut Secret|
     -> Result<WriteDecision<()>, BuildError<Secret>> {
        Err(BuildError::domain(Secret(std::mem::take(&mut input.0))))
    };
    let (result, prepare, commits) =
        observe(|| run(&mut runtime, &mut session, &targets, &mut input, &mut build));
    let BodyOutcome::Rejected(error) = result.body().unwrap() else {
        panic!("{result:?}")
    };
    let original = error.domain_cause().unwrap();
    assert_eq!(original.0.as_ptr(), owner);
    assert_eq!(original.0, CANARY);
    assert_eq!((prepare.calls, prepare.allocations, commits), (0, 0, 0));
    assert_eq!(
        result.diagnostic().category,
        Some(ApplicationCategory::DomainRejected)
    );
    assert!(!format!("{result:?} {error:?} {error}").contains(CANARY));
    assert!(std::error::Error::source(error).is_none());
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn uncalled_mutable_builder_keeps_unique_capture_after_preflight_and_permit_refusal() {
    struct Unique(String);
    for permit_refusal in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let provider = Provider::new();
        let mut session = Session::new(provider.clone());
        session
            .begin_edit(
                runtime.project_fingerprint(),
                targets.session_targets().to_vec(),
            )
            .unwrap();
        provider.reset(usize::from(permit_refusal));
        let snapshot = session.snapshot();
        let wrong_session = crate::data::collaboration_lock::LockSessionId::generate().unwrap();
        let mut capture = Some(Unique(CANARY.to_owned()));
        let owner = capture.as_ref().unwrap().0.as_ptr();
        let mut build =
            move |_: &ArtifactRepository<'_, '_>, _: &mut ()| -> Result<_, BuildError<()>> {
                Ok(WriteDecision::NoWrite(
                    capture.take().expect("builder called once after refusal"),
                ))
            };
        let (result, prepare, commits) = observe(|| {
            execute_write_operation(
                &mut runtime,
                &mut session,
                WriteRequest {
                    project: snapshot.project_fingerprint().unwrap(),
                    session: if permit_refusal {
                        snapshot.session_id().unwrap()
                    } else {
                        &wrong_session
                    },
                    targets: &targets,
                },
                &mut (),
                &mut build,
            )
        });
        assert!(result.body().is_none());
        assert_eq!((prepare.calls, prepare.allocations, commits), (0, 0, 0));
        if permit_refusal {
            provider.reset(0);
            session.resume_edit().unwrap();
        }
        let result = run(&mut runtime, &mut session, &targets, &mut (), &mut build);
        let BodyOutcome::NoWrite(value) = result.body().unwrap() else {
            panic!("{result:?}")
        };
        assert_eq!(value.0.as_ptr(), owner);
        assert_eq!(value.0, CANARY);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
