use super::policies::typed_rich;
use super::*;
use crate::data::{
    artifact::{DocumentMaterializationErrorCategory, DocumentSaveStage},
    repository::{RepositoryCategory, RepositoryOperation},
};

fn source_error(execution: &WriteExecution<(), DocumentUpdateError>, expected: &str) {
    let Some(BodyOutcome::Rejected(error)) = execution.body() else {
        panic!("source rejected before pure calculation")
    };
    let domain = error.domain_cause().expect("original typed source cause");
    let actual = match domain {
        DocumentUpdateError::DocumentSourceMismatch => "document",
        DocumentUpdateError::TemplateSourceMismatch => "template",
        DocumentUpdateError::TemplateRevisionMismatch => "revision",
        DocumentUpdateError::TemplateBindingMismatch => "binding",
        _ => panic!("source precondition must precede pure calculation"),
    };
    assert_eq!(actual, expected);
    assert_eq!(
        execution.diagnostic().category,
        Some(ApplicationCategory::DomainRejected)
    );
}

#[test]
fn g8_stale_revision_raw_sources_foreign_root_and_binding_reject_save_and_materialize() {
    for case in [
        "revision",
        "edited-template",
        "template-raw",
        "document-raw",
        "foreign-document",
        "foreign-template",
        "binding",
        "document-path",
    ] {
        let f = Fixture::new();
        let (tid, did) = seed(&f);
        let mut rt = f.runtime();
        let foreign = Fixture::new();
        seed(&foreign);
        let mut foreign_rt = foreign.runtime();
        let mut input = save_input(
            &mut rt,
            tid,
            did,
            if case == "edited-template" {
                vec![DocumentEdit::SetValue(
                    field(1),
                    DocumentValueEdit::single_choice(option(12)),
                )]
            } else {
                vec![]
            },
        );
        let expected = match case {
            "revision" => {
                input.template.expected_revision = revision(2);
                "revision"
            }
            "edited-template" => {
                mutate(&mut rt, tid, TemplateEditIntent::SetName("new name".into()));
                "template"
            }
            "template-raw" => {
                let mut b = fs::read(template_path(&f, tid)).unwrap();
                b.extend_from_slice(b"\n ");
                fs::write(template_path(&f, tid), b).unwrap();
                "template"
            }
            "document-raw" => {
                let mut b = fs::read(document_path(&f, did)).unwrap();
                b.extend_from_slice(b"\n ");
                fs::write(document_path(&f, did), b).unwrap();
                "document"
            }
            "foreign-document" => {
                input.document = doc_source(&mut foreign_rt, did);
                "document"
            }
            "foreign-template" => {
                input.template = source(&mut foreign_rt, tid);
                "template"
            }
            "binding" => {
                let other = seed_template(&mut rt);
                input.template = source(&mut rt, other);
                "binding"
            }
            "document-path" => {
                let id = DocumentId::new();
                let (_, mut raw) = fixture_raw();
                raw["documentId"] = id.to_string().into();
                fs::write(document_path(&f, id), raw_bytes(&raw)).unwrap();
                input.document.id = id;
                "document"
            }
            _ => unreachable!(),
        };
        let payload = Payload::new();
        let proof = RequestProof::capture(&input, &payload);
        let mat = MaterializeDocumentInput {
            document: DocumentSource {
                id: input.document.id,
                token: input.document.token.clone(),
            },
            template: TemplateSource {
                id: input.template.id,
                token: input.template.token.clone(),
                expected_revision: input.template.expected_revision,
            },
            timestamp_utc: SAVE_TIME.into(),
        };
        let db = disk(&document_path(&f, input.document.id));
        let tb = disk(&template_path(&f, tid));
        let targets = doc_targets(input.document.id);
        let mut session = begin::<Payload>(&rt, targets.session_targets());
        let snap = session.snapshot();
        let ((result, c, k), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(|_| panic!("no namespace on stale source")),
            )),
            false,
            || observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap()),
        );
        no_io(c, k);
        assert_eq!(hooks.hooks, 0);
        assert!(result.outcome().is_none());
        source_error(&result.execution, expected);
        assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
        let (result, c, k) =
            observe(|| materialize_document(&mut rt, &mut session, context(&snap), &mat).unwrap());
        no_io(c, k);
        assert!(result.outcome().is_none());
        source_error(&result.execution, expected);
        assert_disk(&document_path(&f, input.document.id), &db);
        assert_disk(&template_path(&f, tid), &tb);
        assert!(
            mat.document.token == input.document.token
                && mat.template.token == input.template.token
                && mat.timestamp_utc == SAVE_TIME
        );
        proof.assert(&input, &payload);
        proof.drop_payload(payload);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        rt.close().unwrap();
        foreign_rt.close().unwrap();
    }
}

#[test]
fn g8_repository_missing_corrupt_future_and_wrong_id_keep_precise_source_stage() {
    for is_template in [false, true] {
        for case in ["missing", "corrupt", "future", "wrong-id"] {
            let f = Fixture::new();
            let (tid, did) = seed(&f);
            let mut rt = f.runtime();
            let input = save_input(
                &mut rt,
                tid,
                did,
                vec![DocumentEdit::Rename("changed".into())],
            );
            let payload = Payload::new();
            let proof = RequestProof::capture(&input, &payload);
            let path = if is_template {
                template_path(&f, tid)
            } else {
                document_path(&f, did)
            };
            if case == "missing" {
                fs::remove_file(&path).unwrap();
            } else if case == "corrupt" {
                fs::write(&path, b"{").unwrap();
            } else {
                let (mut t, mut d) = fixture_raw();
                let raw = if is_template { &mut t } else { &mut d };
                if case == "future" {
                    raw["schemaVersion"] = 99.into();
                } else {
                    raw[if is_template {
                        "templateId"
                    } else {
                        "documentId"
                    }] = key(900).into();
                }
                fs::write(&path, raw_bytes(raw)).unwrap();
            }
            let before_doc = document_path(&f, did)
                .exists()
                .then(|| disk(&document_path(&f, did)));
            let before_template = template_path(&f, tid)
                .exists()
                .then(|| disk(&template_path(&f, tid)));
            let targets = doc_targets(did);
            let mut session = begin::<Payload>(&rt, targets.session_targets());
            let snap = session.snapshot();
            let ((result, c, k), hooks) = repo_hooks::scoped(
                Some((
                    RepositoryStage::Namespace,
                    0,
                    Box::new(|_| panic!("no namespace on invalid source")),
                )),
                false,
                || {
                    observe(|| {
                        save_document(&mut rt, &mut session, context(&snap), &input).unwrap()
                    })
                },
            );
            no_io(c, k);
            assert_eq!(hooks.hooks, 0);
            assert!(result.outcome().is_none());
            let diag = result.execution.diagnostic();
            assert_eq!(diag.category, Some(ApplicationCategory::RepositoryRejected));
            assert_eq!(diag.disk, DiskState::NotAttempted);
            let repo = diag.repository.unwrap();
            assert_eq!(
                repo.operation,
                if is_template {
                    RepositoryOperation::LoadTemplate
                } else {
                    RepositoryOperation::LoadDocument
                }
            );
            assert_eq!(
                repo.category,
                match case {
                    "missing" => RepositoryCategory::NotFound,
                    "wrong-id" => RepositoryCategory::IdMismatch,
                    _ => RepositoryCategory::CodecRejected,
                }
            );
            if case == "future" || case == "corrupt" {
                assert_eq!(repo.stage, RepositoryStage::Decode);
                assert!(repo.codec.is_some());
            }
            for (path, before) in [
                (document_path(&f, did), before_doc),
                (template_path(&f, tid), before_template),
            ] {
                if let Some(before) = before {
                    assert_disk(&path, &before);
                } else {
                    assert!(!path.exists());
                }
            }
            assert_eq!(rt.snapshot().state, RuntimeState::Ready);
            assert_eq!(session.snapshot().state(), EditSessionState::Editing);
            proof.assert(&input, &payload);
            proof.drop_payload(payload);
            session.end_edit().unwrap();
            rt.close().unwrap();
        }
    }
}

#[test]
fn g8_domain_failures_retain_original_typed_error_without_partial_candidate() {
    use DocumentSaveErrorCategory as C;
    use DocumentSaveStage as S;
    for (case, category, stage) in [
        (
            "current-missing",
            C::MissingKnownFieldValue,
            S::Preconditions,
        ),
        ("tombstone", C::TemplateIsTombstoned, S::SourceAdmission),
        (
            "future-binding",
            C::FutureDocumentRevision,
            S::SourceAdmission,
        ),
        ("duplicate-field", C::DuplicateFieldEdit, S::Preconditions),
        ("duplicate-name", C::DuplicateNameEdit, S::Preconditions),
        ("wrong-kind", C::InvalidEditValue, S::Edits),
        ("required-invalid-kind", C::InvalidEditValue, S::Edits),
        ("timestamp", C::InvalidTimestamp, S::Preconditions),
        ("regression", C::TimestampRegression, S::Preconditions),
        ("unknown-field", C::UnknownField, S::Preconditions),
        ("archived-field", C::ArchivedField, S::Preconditions),
        ("unknown-option", C::UnknownOption, S::Edits),
        ("archived-option-added", C::ArchivedOptionAdded, S::Edits),
    ] {
        let f = Fixture::new();
        let (mut t, mut d) = fixture_raw();
        let mut edits = vec![];
        match case {
            "current-missing" => {
                d["fieldValues"].as_object_mut().unwrap().remove(&key(2));
                edits.push(DocumentEdit::SetValue(field(2), typed_rich("repair")));
            }
            "tombstone" => {
                t["lifecycle"] = "deleted".into();
            }
            "future-binding" => {
                d["templateRevision"] = 4.into();
            }
            "duplicate-field" => {
                edits = vec![DocumentEdit::Unset(field(1)), DocumentEdit::Unset(field(1))]
            }
            "duplicate-name" => {
                edits = vec![
                    DocumentEdit::Rename("a".into()),
                    DocumentEdit::Rename("b".into()),
                ]
            }
            "wrong-kind" => edits.push(DocumentEdit::SetValue(
                field(1),
                DocumentValueEdit::number("3".into()),
            )),
            "required-invalid-kind" => {
                t["fields"][key(1)]["required"] = true.into();
                edits.push(DocumentEdit::SetValue(
                    field(1),
                    DocumentValueEdit::number("-".into()),
                ));
            }
            "unknown-field" => edits.push(DocumentEdit::Unset(field(900))),
            "archived-field" => {
                t["fields"][key(2)]["lifecycle"] = "archived".into();
                t["fieldOrder"] = json!([key(1)]);
                edits.push(DocumentEdit::Unset(field(2)));
            }
            "unknown-option" => edits.push(DocumentEdit::SetValue(
                field(1),
                DocumentValueEdit::single_choice(option(900)),
            )),
            "archived-option-added" => {
                d["fieldValues"][key(1)] = json!({"kind":"unset"});
                d["orphanedFieldDefinitions"] = json!({});
                edits.push(DocumentEdit::SetValue(
                    field(1),
                    DocumentValueEdit::single_choice(option(11)),
                ));
            }
            _ => {}
        }
        let (tid, did) = seed_raw(&f, &t, &d);
        let mut rt = f.runtime();
        let mut input = save_input(&mut rt, tid, did, edits);
        if case == "timestamp" {
            input.timestamp_utc = "invalid".into();
        }
        if case == "regression" {
            input.timestamp_utc = "2026-09-01T00:00:00.000Z".into();
        }
        let payload = Payload::new();
        let proof = RequestProof::capture(&input, &payload);
        let db = disk(&document_path(&f, did));
        let tb = disk(&template_path(&f, tid));
        let targets = doc_targets(did);
        let mut session = begin::<Payload>(&rt, targets.session_targets());
        let snap = session.snapshot();
        let (result, c, k) =
            observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
        no_io(c, k);
        assert!(result.outcome().is_none());
        let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
            panic!("domain rejection: {case}")
        };
        let Some(DocumentUpdateError::Save(error)) = error.domain_cause() else {
            panic!("actual pure save cause: {case}")
        };
        assert_eq!(error.category(), category, "case {case}");
        assert_eq!(error.stage(), stage, "case {case}");
        assert!(std::error::Error::source(error).is_none());
        assert!(!format!("{result:?}").contains(PRIVATE));
        assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
        assert_disk(&document_path(&f, did), &db);
        assert_disk(&template_path(&f, tid), &tb);
        proof.assert(&input, &payload);
        proof.drop_payload(payload);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g8_materialize_missing_current_and_timestamp_failures_are_not_unchanged_success() {
    for case in ["current-missing", "timestamp", "regression"] {
        let f = Fixture::new();
        let (t, mut d) = fixture_raw();
        if case == "current-missing" {
            d["fieldValues"].as_object_mut().unwrap().remove(&key(2));
        }
        let (tid, did) = seed_raw(&f, &t, &d);
        let mut rt = f.runtime();
        let mut input = mat_input(&mut rt, tid, did);
        if case == "timestamp" {
            input.timestamp_utc = "invalid".into();
        }
        if case == "regression" {
            input.timestamp_utc = "2026-09-01T00:00:00.000Z".into();
        }
        let before = disk(&document_path(&f, did));
        let token = input.document.token.clone();
        let time_ptr = input.timestamp_utc.as_ptr();
        let targets = doc_targets(did);
        let mut session: Session = begin(&rt, targets.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            materialize_document(&mut rt, &mut session, context(&snap), &input).unwrap()
        });
        no_io(c, k);
        assert!(result.outcome().is_none());
        let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
            panic!("materialize rejection")
        };
        let Some(DocumentUpdateError::Materialization(error)) = error.domain_cause() else {
            panic!("materialize original cause")
        };
        assert_eq!(
            error.category(),
            match case {
                "timestamp" => DocumentMaterializationErrorCategory::InvalidTimestamp,
                "regression" => DocumentMaterializationErrorCategory::TimestampRegression,
                _ => DocumentMaterializationErrorCategory::BlockingIssues,
            }
        );
        assert!(input.document.token == token && input.timestamp_utc.as_ptr() == time_ptr);
        assert_disk(&document_path(&f, did), &before);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}
