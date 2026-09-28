use super::*;
use crate::data::artifact::template_mutation::TemplateMutationErrorCategory as TC;
use crate::data::repository::{RepositoryCategory, RepositoryOperation};

fn keep() -> TemplateEditIntent {
    TemplateEditIntent::KeepCurrentDefault { field: field(4) }
}
fn absent(result: &CompositeSaveExecution) {
    assert!(result.template_outcome().is_none() && result.document_outcome().is_none());
    assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
}

#[test]
fn g9_keep_no_write_and_partial_no_op_rejection_preserve_pair_then_explicit_g8_still_saves() {
    use crate::data::application::documents::persistence::{save_document, SaveDocumentInput};
    let f = Fixture::new();
    let (t, d) = fixture_raw();
    f.seed(&t, &d);
    let mut rt = f.runtime();
    let before = pair(&f);
    preserved(&before.template.0, &before.document.0);
    for changed in [false, true] {
        let input = load_input(
            &mut rt,
            keep(),
            if changed {
                vec![DocumentEdit::Rename("changed".into())]
            } else {
                vec![]
            },
        );
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        for _ in 0..2 {
            let (result, c, k) = observe(|| {
                update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
            });
            no_io(c, k);
            assert!(matches!(
                result.template_outcome(),
                Some(TemplateMutationOutcome::Unchanged)
            ));
            let doc = result.document_outcome().unwrap();
            warning(doc);
            if changed {
                assert!(matches!(
                    domain(&result),
                    CompositeSaveError::TemplateUnchangedDocumentChanged
                ));
                assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
                assert_eq!(doc.kind(), DocumentSaveOutcomeKind::Changed);
                assert!(
                    doc.document().name() == "changed" && doc.document().updated_at_utc() == SAVE
                );
            } else {
                assert!(matches!(
                    result.execution.body(),
                    Some(BodyOutcome::NoWrite(()))
                ));
                assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
                assert_eq!(doc.kind(), DocumentSaveOutcomeKind::Unchanged);
                assert!(doc.document().updated_at_utc() == TIME);
            }
            assert_eq!(doc.document().template_revision(), revision(3));
            same_pair(&f, &before);
            assert!(session.snapshot().targets() == snap.targets());
            assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        }
        session.end_edit().unwrap();
        if changed {
            let single = SaveDocumentInput {
                template: input.template,
                document: input.document,
                edits: input.edits,
                timestamp_utc: input.timestamp_utc,
            };
            let targets = ExactWriteTargets::new([ArtifactSourceId::Document(did())]).unwrap();
            let mut session = begin(&rt, targets.session_targets());
            let snap = session.snapshot();
            let (result, c, k) =
                observe(|| save_document(&mut rt, &mut session, context(&snap), &single).unwrap());
            assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
            assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
            assert!(disk(&f.template_path()) == before.template);
            assert!(disk(&f.sentinel_path()) == before.sentinel);
            assert!(
                fs::read(f.document_path()).unwrap()
                    == artifact::encode_document(result.outcome().unwrap().document()).unwrap()
            );
            session.end_edit().unwrap();
        }
    }
    rt.close().unwrap();
}

#[test]
fn g9_source_revision_raw_root_id_and_binding_checks_precede_even_no_op_pure_work() {
    for noop in [false, true] {
        for case in [
            "normal",
            "revision",
            "template-raw",
            "document-raw",
            "template-external",
            "template-root",
            "document-root",
            "document-path",
            "binding",
        ] {
            let f = Fixture::new();
            let (t, mut d) = fixture_raw();
            if case == "binding" {
                d["templateId"] = key(101).into();
            }
            f.seed(&t, &d);
            let foreign = Fixture::new();
            foreign.seed(&t, &d);
            let mut other = foreign.runtime();
            let foreign_input = load_input(&mut other, keep(), vec![]);
            let mut rt = f.runtime();
            let mut input = load_input(
                &mut rt,
                if noop {
                    keep()
                } else {
                    TemplateEditIntent::SetName("changed".into())
                },
                vec![],
            );
            match case {
                "revision" => input.template.expected_revision = revision(2),
                "template-raw" | "document-raw" => {
                    let p = if case == "template-raw" {
                        f.template_path()
                    } else {
                        f.document_path()
                    };
                    let mut b = fs::read(&p).unwrap();
                    b.push(b' ');
                    fs::write(p, b).unwrap();
                }
                "template-external" => {
                    let g6 = UpdateTemplateInput {
                        source: template_source(&mut rt),
                        intent: TemplateEditIntent::SetName("external".into()),
                        timestamp_utc: LATER.into(),
                    };
                    let targets =
                        ExactWriteTargets::new([ArtifactSourceId::Template(tid())]).unwrap();
                    let mut s = begin(&rt, targets.session_targets());
                    let snap = s.snapshot();
                    let r = update_template(&mut rt, &mut s, context(&snap), &g6).unwrap();
                    assert_eq!(r.execution.diagnostic().disk, DiskState::Committed);
                    s.end_edit().unwrap();
                }
                "template-root" => input.template = foreign_input.template,
                "document-root" => input.document = foreign_input.document,
                "document-path" => input.document.id = sentinel_id(),
                _ => {}
            }
            let before = pair(&f);
            let request = CompositeWriteRequest::new(&input).unwrap();
            let mut session = begin(&rt, request.session_targets());
            let snap = session.snapshot();
            let ((result, c, k), hooks) = repo_hooks::scoped(
                Some((
                    RepositoryStage::Namespace,
                    0,
                    Box::new(|_| panic!("Replace never creates namespace")),
                )),
                false,
                || {
                    observe(|| {
                        update_template_and_save_document(
                            &mut rt,
                            &mut session,
                            context(&snap),
                            &request,
                        )
                    })
                },
            );
            assert_eq!(hooks.hooks, 0);
            if case == "normal" && !noop {
                assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
                assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
                assert_eq!(
                    result
                        .template_outcome()
                        .unwrap()
                        .changed()
                        .unwrap()
                        .revision(),
                    revision(4)
                );
                assert_eq!(
                    result
                        .document_outcome()
                        .unwrap()
                        .document()
                        .template_revision(),
                    revision(4)
                );
            } else if case == "normal" {
                no_io(c, k);
                assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
                same_pair(&f, &before);
            } else {
                no_io(c, k);
                absent(&result);
                let CompositeSaveError::Source(cause) = domain(&result) else {
                    panic!("actual source rejection")
                };
                assert!(
                    matches!(
                        (case, cause),
                        ("revision", DocumentUpdateError::TemplateRevisionMismatch)
                            | (
                                "template-raw" | "template-external" | "template-root",
                                DocumentUpdateError::TemplateSourceMismatch
                            )
                            | (
                                "document-raw" | "document-root" | "document-path",
                                DocumentUpdateError::DocumentSourceMismatch
                            )
                            | ("binding", DocumentUpdateError::TemplateBindingMismatch)
                    ),
                    "specific source condition"
                );
                same_pair(&f, &before);
            }
            assert_eq!(rt.snapshot().state, RuntimeState::Ready);
            session.end_edit().unwrap();
            rt.close().unwrap();
            other.close().unwrap();
        }
    }
}

#[test]
fn g9_original_future_and_current_missing_are_not_hidden_by_prospective_revision_or_archive() {
    for case in ["normal", "future", "missing"] {
        let f = Fixture::new();
        let (t, mut d) = fixture_raw();
        if case == "future" {
            d["templateRevision"] = 4.into();
        }
        if case == "missing" {
            d["fieldValues"].as_object_mut().unwrap().remove(&key(4));
        }
        f.seed(&t, &d);
        let mut rt = f.runtime();
        let intent = if case == "missing" {
            TemplateEditIntent::ArchiveField(field(4))
        } else {
            TemplateEditIntent::SetName("changed".into())
        };
        let input = load_input(&mut rt, intent, vec![]);
        let before = pair(&f);
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
        });
        if case == "normal" {
            assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
            assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
        } else {
            no_io(c, k);
            absent(&result);
            let CompositeSaveError::OriginalDocument(cause) = domain(&result) else {
                panic!("original guard")
            };
            assert_eq!(
                (cause.category(), cause.stage()),
                if case == "future" {
                    (
                        DocumentSaveErrorCategory::FutureDocumentRevision,
                        DocumentSaveStage::SourceAdmission,
                    )
                } else {
                    (
                        DocumentSaveErrorCategory::MissingKnownFieldValue,
                        DocumentSaveStage::Preconditions,
                    )
                }
            );
            same_pair(&f, &before);
        }
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_missing_corrupt_future_schema_and_wrong_embedded_id_never_create_or_write_other_artifact() {
    for template in [false, true] {
        for case in ["missing", "corrupt", "future", "wrong-id"] {
            let f = Fixture::new();
            let (t, d) = fixture_raw();
            f.seed(&t, &d);
            let mut rt = f.runtime();
            let input = load_input(&mut rt, keep(), vec![]);
            let p = if template {
                f.template_path()
            } else {
                f.document_path()
            };
            if case == "missing" {
                fs::remove_file(&p).unwrap();
            } else if case == "corrupt" {
                fs::write(&p, b"{").unwrap();
            } else {
                let mut raw = if template { t } else { d };
                if case == "future" {
                    raw["schemaVersion"] = 99.into();
                } else {
                    raw[if template { "templateId" } else { "documentId" }] = key(900).into();
                }
                fs::write(&p, raw_bytes(&raw)).unwrap();
            }
            let paths = [f.template_path(), f.document_path(), f.sentinel_path()];
            let before = paths
                .iter()
                .map(|p| p.exists().then(|| disk(p)))
                .collect::<Vec<_>>();
            let request = CompositeWriteRequest::new(&input).unwrap();
            let mut session = begin(&rt, request.session_targets());
            let snap = session.snapshot();
            let ((result, c, k), hooks) = repo_hooks::scoped(
                Some((
                    RepositoryStage::Namespace,
                    0,
                    Box::new(|_| panic!("no Create fallback")),
                )),
                false,
                || {
                    observe(|| {
                        update_template_and_save_document(
                            &mut rt,
                            &mut session,
                            context(&snap),
                            &request,
                        )
                    })
                },
            );
            no_io(c, k);
            absent(&result);
            assert_eq!(hooks.hooks, 0);
            let diag = result.execution.diagnostic();
            assert_eq!(diag.category, Some(ApplicationCategory::RepositoryRejected));
            let repo = diag.repository.unwrap();
            assert_eq!(
                repo.operation,
                if template {
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
            for (p, b) in paths.iter().zip(before) {
                if let Some(b) = b {
                    assert!(disk(p) == b);
                } else {
                    assert!(!p.exists());
                }
            }
            session.end_edit().unwrap();
            rt.close().unwrap();
        }
    }
}

#[test]
fn g9_exact_pair_context_and_actual_pending_are_checked_without_session_repair() {
    for case in [
        "normal",
        "document-only",
        "template-only",
        "extra",
        "project",
        "session",
        "session-project",
        "pending",
    ] {
        let f = Fixture::new();
        let (t, d) = fixture_raw();
        f.seed(&t, &d);
        let mut rt = f.runtime();
        let input = load_input(&mut rt, keep(), vec![]);
        if case == "pending" {
            rt.close().unwrap();
            rt = ProjectRuntime::acquire(&f.root, &f.base.join("locks")).unwrap();
            assert_eq!(rt.snapshot().state, RuntimeState::Pending);
        }
        let foreign = Fixture::new();
        let foreign_rt = foreign.runtime();
        let foreign_project = foreign_rt.project_fingerprint().to_owned();
        foreign_rt.close().unwrap();
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = Session::new(Arc::new(NoLockService::new()));
        let targets = match case {
            "document-only" => vec![ArtifactSourceId::Document(did()).path().unwrap()],
            "template-only" => vec![ArtifactSourceId::Template(tid()).path().unwrap()],
            "extra" => {
                let mut v = request.session_targets().to_vec();
                v.push(ArtifactSourceId::Document(sentinel_id()).path().unwrap());
                v
            }
            _ => request.session_targets().to_vec(),
        };
        session
            .begin_edit(
                if case == "session-project" {
                    &foreign_project
                } else {
                    rt.project_fingerprint()
                },
                targets,
            )
            .unwrap();
        let mut other = begin(&rt, request.session_targets());
        let other_snap = other.snapshot();
        let snap = session.snapshot();
        let before = pair(&f);
        let project = rt.project_fingerprint().to_owned();
        let ctx = TemplateWriteContext {
            project: if case == "project" { "other" } else { &project },
            session: if case == "session" {
                other_snap.session_id().unwrap()
            } else {
                snap.session_id().unwrap()
            },
        };
        let ((result, c, k), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(|_| panic!("no namespace")),
            )),
            false,
            || observe(|| update_template_and_save_document(&mut rt, &mut session, ctx, &request)),
        );
        no_io(c, k);
        assert_eq!(hooks.hooks, 0);
        same_pair(&f, &before);
        if case == "normal" {
            assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
            assert_eq!(hooks.decoded, 2);
        } else {
            absent(&result);
            assert_eq!(hooks.decoded, 0);
            assert_eq!(
                result.execution.diagnostic().category,
                Some(match case {
                    "project" | "session-project" => ApplicationCategory::ProjectMismatch,
                    "session" => ApplicationCategory::SessionMismatch,
                    "pending" => ApplicationCategory::RuntimeRejected,
                    _ => ApplicationCategory::SessionTargetsMismatch,
                })
            );
        }
        assert!(session.snapshot().targets() == snap.targets());
        assert_eq!(session.snapshot().session_id(), snap.session_id());
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        other.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_template_and_document_domain_failures_preserve_original_error_and_correct_candidate_stage() {
    for case in [
        "template-time",
        "template-regression",
        "overflow",
        "deleted",
        "unknown-command",
        "document-regression",
        "wrong-kind",
        "required-unset",
    ] {
        let f = Fixture::new();
        let (mut t, mut d) = fixture_raw();
        if case == "overflow" {
            t["revision"] = u32::MAX.into();
        }
        if case == "deleted" {
            t["lifecycle"] = "deleted".into();
        }
        if case == "document-regression" {
            d["updatedAtUtc"] = AGAIN.into();
        }
        f.seed(&t, &d);
        let mut rt = f.runtime();
        let intent = match case {
            "unknown-command" => TemplateEditIntent::SetFieldLabel {
                field: field(900),
                label: "changed".into(),
            },
            "required-unset" => TemplateEditIntent::SetFieldRequired {
                field: field(4),
                required: true,
            },
            _ => TemplateEditIntent::SetName("changed".into()),
        };
        let edits = match case {
            "wrong-kind" => vec![DocumentEdit::SetValue(
                field(4),
                DocumentValueEdit::single_choice(key(12).parse().unwrap()),
            )],
            "required-unset" => vec![DocumentEdit::Unset(field(4))],
            _ => vec![],
        };
        let mut input = load_input(&mut rt, intent, edits);
        if case == "template-time" {
            input.timestamp_utc = "invalid".into();
        }
        if case == "template-regression" {
            input.timestamp_utc = "2026-09-08T00:00:00.000Z".into();
        }
        let before = pair(&f);
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
        });
        no_io(c, k);
        same_pair(&f, &before);
        assert!(result.document_outcome().is_none());
        match domain(&result) {
            CompositeSaveError::OriginalDocument(e) => {
                assert_eq!(case, "deleted");
                assert_eq!(
                    e.category(),
                    DocumentSaveErrorCategory::TemplateIsTombstoned
                );
                assert!(result.template_outcome().is_none());
            }
            CompositeSaveError::Template(e) => {
                assert_eq!(
                    e.category(),
                    match case {
                        "template-time" => TC::InvalidTimestamp,
                        "template-regression" => TC::TimestampRegression,
                        "overflow" => TC::RevisionOverflow,
                        "unknown-command" => TC::FieldNotFound,
                        _ => panic!("Template failure case"),
                    }
                );
                assert!(result.template_outcome().is_none());
            }
            CompositeSaveError::Document(e) => {
                assert_eq!(
                    (e.category(), e.stage()),
                    if case == "document-regression" {
                        (
                            DocumentSaveErrorCategory::TimestampRegression,
                            DocumentSaveStage::Preconditions,
                        )
                    } else {
                        (
                            DocumentSaveErrorCategory::InvalidEditValue,
                            DocumentSaveStage::Edits,
                        )
                    }
                );
                assert_eq!(
                    result
                        .template_outcome()
                        .unwrap()
                        .changed()
                        .unwrap()
                        .revision(),
                    revision(4)
                );
            }
            _ => panic!("typed domain error"),
        }
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_keep_uses_final_aggregate_but_external_default_draft_is_not_reauthorized() {
    let f = Fixture::new();
    let (t, d) = fixture_raw();
    f.seed(&t, &d);
    let mut rt = f.runtime();
    let before = pair(&f);
    let draft = {
        let ready = rt.ready().unwrap();
        let repo = ArtifactRepository::new(&ready).unwrap();
        let loaded = repo.load_template(tid()).unwrap();
        let draft = loaded.artifact().current_default_draft(field(4)).unwrap();
        let command = artifact::template_mutation::TemplateMutationCommand::set_current_default(
            field(4),
            draft.clone(),
        );
        let normal = artifact::template_mutation::apply_template_mutation(
            loaded.artifact(),
            revision(3),
            SAVE,
            command,
        )
        .unwrap();
        assert!(normal.is_unchanged());
        draft
    };
    for external in [false, true] {
        let input = load_input(
            &mut rt,
            if external {
                TemplateEditIntent::SetCurrentDefault {
                    field: field(4),
                    value: draft.clone(),
                }
            } else {
                keep()
            },
            vec![],
        );
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
        });
        no_io(c, k);
        same_pair(&f, &before);
        if external {
            absent(&result);
            let CompositeSaveError::Template(e) = domain(&result) else {
                panic!("draft ownership error")
            };
            assert_eq!(e.category(), TC::DefaultDraftOwnershipMismatch);
        } else {
            assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
            warning(result.document_outcome().unwrap());
        }
        metadata_owner(
            &fs::read(f.template_path()).unwrap(),
            &["fields", &key(4), "defaultValue", "future"],
            "default",
        );
        session.end_edit().unwrap();
    }
    rt.close().unwrap();
}
