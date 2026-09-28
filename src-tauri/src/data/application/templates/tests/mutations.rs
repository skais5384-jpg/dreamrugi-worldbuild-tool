use super::*;
use crate::data::application::write::BodyOutcome;
use crate::data::artifact::{
    template_mutation::{NewFieldConfiguration, TemplateMutationErrorCategory},
    FieldKind, FieldLifecycle, OptionLifecycle,
};
use crate::data::{
    application::diagnostics::{ApplicationCategory, ApplicationStage},
    field_engine::rich_text::normalize_rich_text,
    json::parse_strict_lossless_json_object,
};

pub(super) fn field(n: u32) -> FieldId {
    format!("11111111-1111-4111-8111-{n:012x}").parse().unwrap()
}
pub(super) fn option(n: u32) -> OptionId {
    format!("22222222-2222-4222-8222-{n:012x}").parse().unwrap()
}

#[test]
fn m28_g6_private_metadata_comparisons_keep_failure_without_operands() {
    let fixture = Fixture::new();
    let (_, bytes) = fix001_rich_source(&fixture);
    let left = fix001_ast_metadata(&bytes, 1);
    let right = fix001_ast_metadata(&bytes, 2);
    assert!(left == left.clone());
    assert!(left != right);
    // G7의 boolean assertion/control을 재사용한다. 전역 panic hook은 바꾸지 않는다.
    for failure in [
        std::panic::catch_unwind(|| assert!(left == right)).unwrap_err(),
        std::panic::catch_unwind(|| assert!(left != left.clone())).unwrap_err(),
    ] {
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(message.contains("assertion failed"));
        assert!(!message.contains(CANARY));
        assert!(!message.contains(&format!("{left:?}")));
        assert!(!message.contains(&format!("{right:?}")));
    }
}

// 서로 다른 두 Field의 값과 AST metadata를 구분해야 잘못된 Field lookup도 검출할 수 있다.
fn fix001_rich_source(fixture: &Fixture) -> (TemplateId, Vec<u8>) {
    let mut model = artifact::create_template(CANARY.into(), None, TIME.into()).unwrap();
    for n in [1, 2] {
        let content = serde_json::json!({"kind":"root","children":[{"kind":"paragraph",
            "children":[{"kind":"text","text":format!("{CANARY} field {n}")}]}]});
        let value = FieldValueDraft::from_normalized_rich_text(
            normalize_rich_text(1, content.as_object().unwrap()).unwrap(),
        );
        model = template_mutation::apply_template_mutation(
            &model,
            model.revision(),
            TIME,
            TemplateMutationCommand::create_field(
                NewFieldDraft::new(
                    field(n),
                    CANARY.into(),
                    FieldKind::RichText,
                    NewFieldConfiguration::rich_text(),
                    false,
                    None,
                    value,
                ),
                NewFieldInsertion::Append,
            ),
        )
        .unwrap()
        .into_changed()
        .unwrap();
    }
    let mut raw: serde_json::Value =
        serde_json::from_slice(&artifact::encode_template(&model).unwrap()).unwrap();
    for (n, marker) in [(1, "FIX_NUMBER_ONE"), (2, "FIX_NUMBER_TWO")] {
        raw["fields"][field(n).to_string()]["defaultValue"]["document"]["content"]["futureAudit"] =
            serde_json::json!({"n":marker,"owner":n,"private":CANARY});
    }
    let bytes = serde_json::to_string(&raw)
        .unwrap()
        .replace("\"FIX_NUMBER_ONE\"", "7E+109")
        .replace("\"FIX_NUMBER_TWO\"", "8E+108")
        .into_bytes();
    assert!(artifact::decode_template(&bytes).is_ok());
    fs::create_dir(fixture.root.join("templates")).unwrap();
    fs::write(path(fixture, model.template_id()), &bytes).unwrap();
    (model.template_id(), bytes)
}

fn fix001_ast_metadata(bytes: &[u8], n: u32) -> Vec<u8> {
    let source = parse_strict_lossless_json_object(bytes).unwrap();
    let id = field(n).to_string();
    let owner = source
        .object_path(&[
            "fields",
            &id,
            "defaultValue",
            "document",
            "content",
            "futureAudit",
        ])
        .unwrap();
    serde_json::to_vec(owner).unwrap()
}

fn fix001_assert_rejected(
    result: &TemplateMutationExecution,
    expected: TemplateMutationErrorCategory,
) {
    let d = result.execution.diagnostic();
    assert_eq!(d.disk, DiskState::NotAttempted);
    assert_eq!(d.stage, ApplicationStage::Build);
    assert_eq!(d.category, Some(ApplicationCategory::DomainRejected));
    assert!(result.candidate().is_none());
    let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
        panic!("expected retained domain error")
    };
    let Some(TemplateUseCaseError::Mutation(error)) = error.domain_cause() else {
        panic!("expected retained mutation cause")
    };
    assert_eq!(error.category(), expected);
}

#[test]
fn fix001_same_source_pure_control_and_external_draft_rejection() {
    let fixture = Fixture::new();
    let (id, bytes) = fix001_rich_source(&fixture);
    let metadata = fix001_ast_metadata(&bytes, 1);
    assert!(metadata.windows(6).any(|v| v == b"7E+109"));
    assert!(metadata != fix001_ast_metadata(&bytes, 2));
    let mut runtime = fixture.runtime();
    let ready = runtime.ready().unwrap();
    let repository = ArtifactRepository::new(&ready).unwrap();
    let loaded = repository.load_template(id).unwrap();
    let own = loaded.artifact().current_default_draft(field(1)).unwrap();
    let pure = template_mutation::apply_template_mutation(
        loaded.artifact(),
        loaded.artifact().revision(),
        LATER,
        TemplateMutationCommand::set_current_default(field(1), own.clone()),
    )
    .unwrap();
    assert!(pure.is_unchanged());
    assert!(repository.load_template(id).unwrap().source() == loaded.source());
    let src = TemplateSource {
        id,
        token: loaded.source().clone(),
        expected_revision: loaded.artifact().revision(),
    };
    drop(repository);
    drop(ready);
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(&runtime, targets.session_targets());
    let snap = session.snapshot();
    let mtime = fs::metadata(path(&fixture, id))
        .unwrap()
        .modified()
        .unwrap();
    let wrong_field = loaded.artifact().current_default_draft(field(2)).unwrap();
    let pure_wrong_field = template_mutation::apply_template_mutation(
        loaded.artifact(),
        src.expected_revision,
        LATER,
        TemplateMutationCommand::set_current_default(field(1), wrong_field.clone()),
    )
    .unwrap_err();
    assert_eq!(
        pure_wrong_field.category(),
        TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch
    );
    let mut foreign_raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    foreign_raw["templateId"] = TemplateId::new().to_string().into();
    let foreign = artifact::decode_template(&serde_json::to_vec(&foreign_raw).unwrap()).unwrap();
    // 같은 token이어도 외부 draft는 다른 decode의 snapshot이므로 Keep으로 자동 변환하지 않는다.
    for value in [
        own,
        wrong_field,
        foreign.current_default_draft(field(1)).unwrap(),
    ] {
        let input = UpdateTemplateInput {
            source: TemplateSource {
                id,
                token: src.token.clone(),
                expected_revision: src.expected_revision,
            },
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::SetCurrentDefault {
                field: field(1),
                value,
            },
        };
        let (result, counts, commits) = observe(|| {
            update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
        });
        assert_no_io(&result.execution, counts, commits);
        fix001_assert_rejected(
            &result,
            TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch,
        );
        assert!(fs::read(path(&fixture, id)).unwrap() == bytes);
        assert_eq!(
            fs::metadata(path(&fixture, id))
                .unwrap()
                .modified()
                .unwrap(),
            mtime
        );
    }
    assert_eq!(load(&mut runtime, id).revision(), src.expected_revision);
    assert_eq!(load(&mut runtime, id).updated_at_utc(), TIME);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn fix001_keep_rich_default_repeats_lossless_nowrite_for_each_field() {
    let fixture = Fixture::new();
    let (id, bytes) = fix001_rich_source(&fixture);
    let metadata: Vec<_> = [1, 2].map(|n| fix001_ast_metadata(&bytes, n)).into();
    assert!(metadata[0].windows(6).any(|v| v == b"7E+109"));
    assert!(metadata[1].windows(6).any(|v| v == b"8E+108"));
    assert!(metadata[0] != metadata[1]);
    let mut runtime = fixture.runtime();
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(&runtime, targets.session_targets());
    let snap = session.snapshot();
    let mtime = fs::metadata(path(&fixture, id))
        .unwrap()
        .modified()
        .unwrap();
    let transaction_dir = fixture.root.join(".worldbuild/transactions").exists();
    for n in [1, 2] {
        let input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::KeepCurrentDefault { field: field(n) },
        };
        let token = input.source.token.clone();
        for _ in 0..3 {
            let (result, counts, commits) = observe(|| {
                update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
            });
            assert_no_io(&result.execution, counts, commits);
            assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
            assert_eq!(
                result.execution.diagnostic().stage,
                ApplicationStage::NoWrite
            );
            assert!(result.candidate().is_none());
            let after = fs::read(path(&fixture, id)).unwrap();
            assert!(after == bytes);
            for (n, expected) in [1, 2].into_iter().zip(&metadata) {
                assert!(fix001_ast_metadata(&after, n) == *expected);
            }
            assert_eq!(
                fs::metadata(path(&fixture, id))
                    .unwrap()
                    .modified()
                    .unwrap(),
                mtime
            );
            let saved = load(&mut runtime, id);
            assert_eq!(saved.revision(), input.source.expected_revision);
            assert_eq!(saved.updated_at_utc(), TIME);
            assert!(source(&mut runtime, id).token == token);
            assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
            assert_eq!(session.snapshot().state(), EditSessionState::Editing);
            assert_eq!(journals(&fixture), 0);
            assert_eq!(
                fixture.root.join(".worldbuild/transactions").exists(),
                transaction_dir
            );
            assert!(!fixture.root.join("documents").exists());
            assert_eq!(
                fs::read_dir(fixture.root.join("templates"))
                    .unwrap()
                    .count(),
                1
            );
        }
    }
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn fix001_keep_uses_requested_field_and_rejects_archived_or_absent_field() {
    for selected in [1, 2] {
        let fixture = Fixture::new();
        let (id, bytes) = fix001_rich_source(&fixture);
        let original = artifact::decode_template(&bytes).unwrap();
        let archived = 3 - selected;
        let model = template_mutation::apply_template_mutation(
            &original,
            original.revision(),
            TIME,
            TemplateMutationCommand::archive_field(field(archived)),
        )
        .unwrap()
        .into_changed()
        .unwrap();
        install(&fixture, &model);
        let before = fs::read(path(&fixture, id)).unwrap();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
        let mut session: Session = begin(&runtime, targets.session_targets());
        let snap = session.snapshot();
        // 다른 Field는 archived라서 임의의 첫 Field를 택하는 구현은 정상 control을 통과할 수 없다.
        for (n, expected) in [
            (selected, None),
            (
                archived,
                Some(TemplateMutationErrorCategory::FieldIsArchived),
            ),
            (99, Some(TemplateMutationErrorCategory::FieldNotFound)),
        ] {
            let input = UpdateTemplateInput {
                source: source(&mut runtime, id),
                timestamp_utc: LATER.into(),
                intent: TemplateEditIntent::KeepCurrentDefault { field: field(n) },
            };
            let (result, counts, commits) = observe(|| {
                update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
            });
            assert_no_io(&result.execution, counts, commits);
            if let Some(expected) = expected {
                fix001_assert_rejected(&result, expected);
            } else {
                assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
            }
            assert!(fs::read(path(&fixture, id)).unwrap() == before);
            assert!(
                fix001_ast_metadata(&before, selected) == fix001_ast_metadata(&bytes, selected)
            );
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn fix001_keep_preconditions_retain_input_nonclone_payload_and_redacted_errors() {
    struct NonClone(String);
    for case in [
        "raw-stale",
        "wrong-template",
        "wrong-root",
        "revision",
        "invalid-time",
        "time-regression",
        "deleted",
    ] {
        let fixture = Fixture::new();
        let (id, bytes) = fix001_rich_source(&fixture);
        if case == "deleted" {
            let mut raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            raw["lifecycle"] = "deleted".into();
            fs::write(path(&fixture, id), serde_json::to_vec(&raw).unwrap()).unwrap();
        }
        let mut runtime = fixture.runtime();
        let mut input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::KeepCurrentDefault { field: field(1) },
        };
        match case {
            "raw-stale" => {
                let mut changed = bytes.clone();
                changed.push(b' ');
                fs::write(path(&fixture, id), changed).unwrap();
            }
            "wrong-template" => {
                let other =
                    artifact::duplicate_template(&load(&mut runtime, id), TIME.into()).unwrap();
                install(&fixture, &other);
                input.source.id = other.template_id();
                input.source.expected_revision = other.revision();
            }
            "wrong-root" => {
                let foreign = Fixture::new();
                fs::create_dir(foreign.root.join("templates")).unwrap();
                fs::write(path(&foreign, id), &bytes).unwrap();
                let mut foreign_runtime = foreign.runtime();
                input.source.token = source(&mut foreign_runtime, id).token;
                foreign_runtime.close().unwrap();
            }
            "revision" => {
                input.source.expected_revision =
                    input.source.expected_revision.checked_increment().unwrap();
            }
            "invalid-time" => {
                input.timestamp_utc = CANARY.into();
            }
            "time-regression" => {
                input.timestamp_utc = "2026-09-09T00:00:00.000Z".into();
            }
            "deleted" => {}
            _ => unreachable!(),
        }
        let before = fs::read(path(&fixture, id)).unwrap();
        let mtime = fs::metadata(path(&fixture, id))
            .unwrap()
            .modified()
            .unwrap();
        let targets =
            ExactWriteTargets::new([ArtifactSourceId::Template(input.source.id)]).unwrap();
        let mut session = begin::<NonClone>(&runtime, targets.session_targets());
        let snap = session.snapshot();
        let payload = NonClone(CANARY.into());
        let payload_owner = payload.0.as_ptr();
        let intent_owner = &input.intent as *const TemplateEditIntent;
        let timestamp_owner = input.timestamp_utc.as_ptr();
        let token = input.source.token.clone();
        let (result, counts, commits) = observe(|| {
            update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
        });
        assert_no_io(&result.execution, counts, commits);
        let d = result.execution.diagnostic();
        assert_eq!(d.disk, DiskState::NotAttempted, "{case}");
        assert_eq!(d.stage, ApplicationStage::Build);
        assert_eq!(d.category, Some(ApplicationCategory::DomainRejected));
        let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
            panic!("{case}")
        };
        match case {
            "raw-stale" | "wrong-template" | "wrong-root" => assert!(matches!(
                error.domain_cause(),
                Some(TemplateUseCaseError::SourceMismatch)
            )),
            "revision" => assert!(matches!(
                error.domain_cause(),
                Some(TemplateUseCaseError::RevisionMismatch)
            )),
            "invalid-time" => {
                fix001_assert_rejected(&result, TemplateMutationErrorCategory::InvalidTimestamp)
            }
            "time-regression" => {
                fix001_assert_rejected(&result, TemplateMutationErrorCategory::TimestampRegression)
            }
            "deleted" => {
                fix001_assert_rejected(&result, TemplateMutationErrorCategory::TemplateIsTombstoned)
            }
            _ => unreachable!(),
        }
        let cause = error.domain_cause().unwrap();
        let cause_owner = cause as *const TemplateUseCaseError;
        let diagnostic = format!("{input:?} {result:?} {d:?} {cause:?} {cause}");
        for private in [
            CANARY,
            "7E+109",
            "futureAudit",
            fixture.root.to_str().unwrap(),
        ] {
            assert!(!diagnostic.contains(private));
        }
        assert!(!d.next_action().is_empty());
        assert_eq!(
            cause_owner,
            error.domain_cause().unwrap() as *const TemplateUseCaseError
        );
        assert_eq!(payload_owner, payload.0.as_ptr());
        assert!(payload.0.as_str() == CANARY);
        assert_eq!(intent_owner, &input.intent as *const TemplateEditIntent);
        assert_eq!(timestamp_owner, input.timestamp_utc.as_ptr());
        assert!(input.source.token == token);
        assert!(
            matches!(input.intent, TemplateEditIntent::KeepCurrentDefault {field: f} if f == field(1))
        );
        assert!(fs::read(path(&fixture, id)).unwrap() == before);
        assert_eq!(
            fs::metadata(path(&fixture, id))
                .unwrap()
                .modified()
                .unwrap(),
            mtime
        );
        assert_eq!(journals(&fixture), 0);
        assert!(result.candidate().is_none());
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn fix001_fresh_default_change_then_noop_preserves_exact_outer_owner() {
    let fixture = Fixture::new();
    let model = defined();
    let id = model.template_id();
    let mut raw: serde_json::Value =
        serde_json::from_slice(&artifact::encode_template(&model).unwrap()).unwrap();
    raw["fields"][field(2).to_string()]["defaultValue"]["auditOuter"] =
        serde_json::json!({"n":"FIX_NUMBER"});
    let bytes = serde_json::to_string(&raw)
        .unwrap()
        .replace("\"FIX_NUMBER\"", "9e+117")
        .into_bytes();
    install(&fixture, &model);
    fs::write(path(&fixture, id), &bytes).unwrap();
    let owner = |bytes: &[u8]| {
        let source = parse_strict_lossless_json_object(bytes).unwrap();
        serde_json::to_vec(
            source
                .object_path(&[
                    "fields",
                    &field(2).to_string(),
                    "defaultValue",
                    "auditOuter",
                ])
                .unwrap(),
        )
        .unwrap()
    };
    let expected = owner(&bytes);
    assert!(expected.windows(6).any(|v| v == b"9e+117"));
    let mut runtime = fixture.runtime();
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(&runtime, targets.session_targets());
    let snap = session.snapshot();
    for changed in [true, false] {
        let before = fs::read(path(&fixture, id)).unwrap();
        let mtime = fs::metadata(path(&fixture, id))
            .unwrap()
            .modified()
            .unwrap();
        let input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::SetCurrentDefault {
                field: field(2),
                value: FieldValueDraft::single_line_text("fresh replacement".into()),
            },
        };
        let (result, counts, commits) = observe(|| {
            update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
        });
        if changed {
            assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
            assert_eq!((counts.calls, counts.allocations, commits), (1, 1, 1));
        } else {
            assert_no_io(&result.execution, counts, commits);
            assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
            assert!(fs::read(path(&fixture, id)).unwrap() == before);
            assert_eq!(
                fs::metadata(path(&fixture, id))
                    .unwrap()
                    .modified()
                    .unwrap(),
                mtime
            );
        }
        let after = fs::read(path(&fixture, id)).unwrap();
        assert!(owner(&after) == expected);
        let saved = load(&mut runtime, id);
        assert_eq!(
            saved.revision(),
            model.revision().checked_increment().unwrap()
        );
        assert_eq!(saved.updated_at_utc(), LATER);
        assert_eq!(
            saved.fields()[&field(2)].default_value().text(),
            Some("fresh replacement")
        );
        assert_eq!(
            saved.fields()[&field(2)].initial_default_value(),
            model.fields()[&field(2)].initial_default_value()
        );
    }
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
fn draft(n: u32) -> NewFieldDraft {
    NewFieldDraft::new(
        field(n),
        "text".into(),
        FieldKind::SingleLineText,
        NewFieldConfiguration::single_line_text(),
        false,
        None,
        FieldValueDraft::single_line_text("initial".into()),
    )
}
pub(super) fn defined() -> TemplateArtifact {
    let empty = artifact::create_template(CANARY.into(), None, TIME.into()).unwrap();
    let choice = NewFieldDraft::new(
        field(1),
        "choice".into(),
        FieldKind::SingleChoice,
        NewFieldConfiguration::single_choice(
            vec![option(1), option(2)],
            vec![
                NewChoiceOptionDraft::new(option(1), "one".into()),
                NewChoiceOptionDraft::new(option(2), "two".into()),
            ],
        ),
        false,
        None,
        FieldValueDraft::single_choice(option(1)),
    );
    let one = template_mutation::apply_template_mutation(
        &empty,
        empty.revision(),
        TIME,
        TemplateMutationCommand::create_field(choice, NewFieldInsertion::Append),
    )
    .unwrap()
    .into_changed()
    .unwrap();
    template_mutation::apply_template_mutation(
        &one,
        one.revision(),
        TIME,
        TemplateMutationCommand::create_field(draft(2), NewFieldInsertion::Append),
    )
    .unwrap()
    .into_changed()
    .unwrap()
}
pub(super) fn install(fixture: &Fixture, value: &TemplateArtifact) {
    fs::create_dir_all(fixture.root.join("templates")).unwrap();
    fs::write(
        path(fixture, value.template_id()),
        artifact::encode_template(value).unwrap(),
    )
    .unwrap();
}
pub(super) fn document(fixture: &Fixture, template: TemplateId) -> (PathBuf, Vec<u8>) {
    fs::create_dir_all(fixture.root.join("documents")).unwrap();
    let id = artifact::DocumentId::new();
    let value = serde_json::json!({"artifactType":"document","schemaVersion":1,"documentId":id.to_string(),"templateId":template.to_string(),"templateRevision":1,"name":CANARY,"fieldValues":{},"orphanedFieldDefinitions":{},"createdAtUtc":TIME,"updatedAtUtc":TIME});
    let bytes = serde_json::to_vec(&value).unwrap();
    assert!(artifact::decode_document(&bytes).is_ok());
    let p = fixture.root.join("documents").join(format!("{id}.json"));
    fs::write(&p, &bytes).unwrap();
    (p, bytes)
}
#[test]
fn g6_update_dispatch_connects_every_closed_command_and_preserves_documents() {
    let cases = vec![
        TemplateEditIntent::SetName("changed".into()),
        TemplateEditIntent::SetPresentationToken(Some("token".into())),
        TemplateEditIntent::CreateField {
            draft: draft(3),
            insertion: NewFieldInsertion::At(0),
        },
        TemplateEditIntent::SetFieldLabel {
            field: field(2),
            label: "label".into(),
        },
        TemplateEditIntent::SetFieldRequired {
            field: field(2),
            required: true,
        },
        TemplateEditIntent::SetFieldPresentationToken {
            field: field(2),
            token: Some("token".into()),
        },
        TemplateEditIntent::SetCurrentDefault {
            field: field(2),
            value: FieldValueDraft::single_line_text("new".into()),
        },
        TemplateEditIntent::ReorderFields(vec![field(2), field(1)]),
        TemplateEditIntent::ArchiveField(field(2)),
        TemplateEditIntent::AddOption {
            field: field(1),
            draft: NewChoiceOptionDraft::new(option(3), "three".into()),
            insertion: NewOptionInsertion::At(0),
        },
        TemplateEditIntent::RenameOption {
            field: field(1),
            option: option(1),
            label: "renamed".into(),
        },
        TemplateEditIntent::ReorderOptions {
            field: field(1),
            order: vec![option(2), option(1)],
        },
        TemplateEditIntent::ArchiveOption {
            field: field(1),
            option: option(1),
            repair: Some(FieldValueDraft::single_choice(option(2))),
        },
    ];
    for (case, intent) in cases.into_iter().enumerate() {
        let fixture = Fixture::new();
        let old = defined();
        let id = old.template_id();
        install(&fixture, &old);
        let (doc, doc_bytes) = document(&fixture, id);
        let mut runtime = fixture.runtime();
        let input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent,
        };
        let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
        let mut session: Session = begin(&runtime, targets.session_targets());
        let snap = session.snapshot();
        let (result, counts, commits) = observe(|| {
            update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
        });
        assert_eq!(
            result.execution.diagnostic().disk,
            DiskState::Committed,
            "case {case}: {result:?}"
        );
        assert_eq!((counts.calls, counts.allocations, commits), (1, 1, 1));
        let saved = load(&mut runtime, id);
        assert_eq!(
            saved.revision(),
            old.revision().checked_increment().unwrap()
        );
        assert_eq!(saved.updated_at_utc(), LATER);
        assert!(result.candidate().is_some());
        let text = &saved.fields()[&field(2)];
        let choice = &saved.fields()[&field(1)];
        match case {
            0 => assert!(saved.name() == "changed"),
            1 => assert!(saved.presentation().token() == Some("token")),
            2 => assert_eq!(saved.field_order()[0], field(3)),
            3 => assert!(text.label() == "label"),
            4 => assert!(text.required()),
            5 => assert!(text.presentation().token() == Some("token")),
            6 => assert!(text.default_value().text() == Some("new")),
            7 => assert_eq!(saved.field_order(), &[field(2), field(1)]),
            8 => assert_eq!(text.lifecycle(), FieldLifecycle::Archived),
            9 => assert_eq!(choice.configuration().option_order().unwrap()[0], option(3)),
            10 => {
                assert!(choice.configuration().options().unwrap()[&option(1)].label() == "renamed")
            }
            11 => assert_eq!(
                choice.configuration().option_order().unwrap(),
                &[option(2), option(1)]
            ),
            12 => {
                assert_eq!(
                    choice.configuration().options().unwrap()[&option(1)].lifecycle(),
                    OptionLifecycle::Archived
                );
                assert_eq!(choice.default_value().single_choice(), Some(option(2)));
            }
            _ => unreachable!(),
        }
        assert!(text.initial_default_value() == old.fields()[&field(2)].initial_default_value());
        assert!(choice.initial_default_value() == old.fields()[&field(1)].initial_default_value());
        assert!(fs::read(doc).unwrap() == doc_bytes);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
#[test]
fn g6_noop_stale_source_revision_time_and_domain_rejections_keep_bytes_and_intent() {
    let fixture = Fixture::new();
    let old = defined();
    let id = old.template_id();
    install(&fixture, &old);
    let mut runtime = fixture.runtime();
    let original = fs::read(path(&fixture, id)).unwrap();
    let mtime = fs::metadata(path(&fixture, id))
        .unwrap()
        .modified()
        .unwrap();
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(&runtime, targets.session_targets());
    let snap = session.snapshot();
    for case in 0..8 {
        let mut input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::SetName(old.name().into()),
        };
        match case {
            1 => input.source.expected_revision = TemplateRevision::INITIAL,
            2 => input.timestamp_utc = "invalid-time".into(),
            3 => input.timestamp_utc = "2020-01-01T00:00:00.000Z".into(),
            4 => {
                input.intent = TemplateEditIntent::SetFieldLabel {
                    field: field(99),
                    label: CANARY.into(),
                };
            }
            5 => {
                input.intent = TemplateEditIntent::SetCurrentDefault {
                    field: field(2),
                    value: old.current_default_draft(field(2)).unwrap(),
                }
            }
            6 => {
                let mut changed = original.clone();
                changed.push(b' ');
                fs::write(path(&fixture, id), changed).unwrap();
            }
            7 => {
                input.intent = TemplateEditIntent::ArchiveOption {
                    field: field(1),
                    option: option(1),
                    repair: None,
                }
            }
            _ => {}
        }
        let ptr = input.timestamp_utc.as_ptr();
        let before = fs::read(path(&fixture, id)).unwrap();
        let (result, counts, commits) = observe(|| {
            update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
        });
        assert_no_io(&result.execution, counts, commits);
        assert!(fs::read(path(&fixture, id)).unwrap() == before);
        assert_eq!(ptr, input.timestamp_utc.as_ptr());
        assert!(result.candidate().is_none());
        if case == 0 {
            assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
            assert_eq!(
                fs::metadata(path(&fixture, id))
                    .unwrap()
                    .modified()
                    .unwrap(),
                mtime
            );
        } else {
            assert!(matches!(
                result.execution.body(),
                Some(BodyOutcome::Rejected(_))
            ));
        }
        if case == 5 {
            let Some(BodyOutcome::Rejected(e)) = result.execution.body() else {
                panic!()
            };
            assert!(
                matches!(e.domain_cause(),Some(TemplateUseCaseError::Mutation(e)) if e.category()==TemplateMutationErrorCategory::DefaultDraftOwnershipMismatch)
            );
        }
        if case == 6 {
            fs::write(path(&fixture, id), &original).unwrap();
        }
    }
    assert_eq!(load(&mut runtime, id).revision(), old.revision());
    assert_eq!(load(&mut runtime, id).updated_at_utc(), TIME);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}
#[test]
fn g6_tombstone_final_scan_sees_new_references_and_late_corruption() {
    for kind in [
        "none",
        "reference",
        "late-corrupt",
        "incomplete",
        "template-changed",
    ] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let id = seed(&mut runtime);
        let src = source(&mut runtime, id);
        {
            let ready = runtime.ready().unwrap();
            let repo = ArtifactRepository::new(&ready).unwrap();
            assert!(repo.assess_template_references(id).is_ok());
        }
        if kind == "reference" {
            document(&fixture, id);
        }
        if kind == "late-corrupt" {
            document(&fixture, TemplateId::new());
            fs::write(
                fixture
                    .root
                    .join("documents/ffffffff-ffff-4fff-8fff-ffffffffffff.json"),
                b"{",
            )
            .unwrap();
        }
        if kind == "incomplete" || kind == "template-changed" {
            fs::create_dir_all(fixture.root.join("documents")).unwrap();
        }
        let input = TombstoneTemplateInput {
            source: src,
            timestamp_utc: LATER.into(),
        };
        let old = fs::read(path(&fixture, id)).unwrap();
        let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
        let mut session: Session = begin(&runtime, targets.session_targets());
        let snap = session.snapshot();
        let target = path(&fixture, id);
        let mut altered = old.clone();
        altered.push(b' ');
        let hook = if kind == "incomplete" || kind == "template-changed" {
            Some((
                RepositoryStage::ReadDirectory,
                0,
                Box::new(move |_: &std::path::Path| {
                    if kind == "incomplete" {
                        Err(io::Error::from_raw_os_error(5))
                    } else {
                        fs::write(&target, &altered)
                    }
                }) as Box<dyn FnMut(&std::path::Path) -> io::Result<()>>,
            ))
        } else {
            None
        };
        let ((result, counts, commits), _) = repository_test::scoped(hook, false, || {
            observe(|| {
                tombstone_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
            })
        });
        if kind == "none" {
            assert_eq!(
                result.execution.diagnostic().disk,
                DiskState::Committed,
                "{result:?}"
            );
            assert_eq!(
                load(&mut runtime, id).lifecycle(),
                artifact::TemplateLifecycle::Deleted
            );
            let again = TombstoneTemplateInput {
                source: source(&mut runtime, id),
                timestamp_utc: LATER.into(),
            };
            let (result, c, k) = observe(|| {
                tombstone_template(&mut runtime, &mut session, context(&snap), &again).unwrap()
            });
            assert_no_io(&result.execution, c, k);
            assert!(matches!(
                result.execution.body(),
                Some(BodyOutcome::Rejected(_))
            ));
        } else {
            assert_no_io(&result.execution, counts, commits);
            assert_eq!(
                load(&mut runtime, id).lifecycle(),
                artifact::TemplateLifecycle::Active
            );
            if kind != "template-changed" {
                assert!(fs::read(path(&fixture, id)).unwrap() == old);
            }
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn g6_tombstone_real_cleanup_failure_pending_recovery_preserves_deleted() {
    use std::cell::RefCell;
    use std::os::windows::fs::OpenOptionsExt;
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let id = seed(&mut runtime);
    let input = TombstoneTemplateInput {
        source: source(&mut runtime, id),
        timestamp_utc: LATER.into(),
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(&runtime, targets.session_targets());
    let snap = session.snapshot();
    let held = Rc::new(RefCell::new(None));
    let held_hook = Rc::clone(&held);
    let reached = Rc::new(Cell::new(0));
    let reached_hook = Rc::clone(&reached);
    let journal_root = fixture.root.join(".worldbuild/transactions");
    let result = with_commit_io_factory(
        move |point, _| {
            if point == Some(CommitTestPoint::Cleanup) {
                let p = fs::read_dir(&journal_root)?
                    .next()
                    .ok_or_else(|| io::Error::other("missing journal"))??;
                let p = p.path();
                assert!(p.join("committed.json").exists());
                reached_hook.set(reached_hook.get() + 1);
                *held_hook.borrow_mut() = Some(
                    fs::OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(p.join("manifest.json"))?,
                );
            }
            Ok(())
        },
        || tombstone_template(&mut runtime, &mut session, context(&snap), &input).unwrap(),
    );
    assert_eq!(reached.get(), 1);
    assert_eq!(
        result.execution.diagnostic().disk,
        DiskState::Committed,
        "{result:?}"
    );
    assert!(result.execution.diagnostic().recovery_required);
    assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
    assert_eq!(journals(&fixture), 1);
    let bytes = fs::read(path(&fixture, id)).unwrap();
    assert_eq!(
        artifact::decode_template(&bytes).unwrap().lifecycle(),
        artifact::TemplateLifecycle::Deleted
    );
    let (again, counts, commits) =
        observe(|| tombstone_template(&mut runtime, &mut session, context(&snap), &input).unwrap());
    assert_no_io(&again.execution, counts, commits);
    assert!(again.execution.body().is_none());
    held.borrow_mut().take();
    runtime.recover().unwrap();
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    assert_eq!(journals(&fixture), 0);
    assert_eq!(
        load(&mut runtime, id).lifecycle(),
        artifact::TemplateLifecycle::Deleted
    );
    assert!(fs::read(path(&fixture, id)).unwrap() == bytes);
    assert_eq!(reached.get(), 1);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g6_update_backup_precondition_and_prepare_errors_keep_candidate_and_input() {
    for changed_source in [false, true] {
        let fixture = Fixture::new();
        let old = defined();
        let id = old.template_id();
        install(&fixture, &old);
        let mut runtime = fixture.runtime();
        let input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::SetName(CANARY.repeat(2)),
        };
        let ptr = match &input.intent {
            TemplateEditIntent::SetName(s) => s.as_ptr(),
            _ => unreachable!(),
        };
        let target = path(&fixture, id);
        let original = fs::read(&target).unwrap();
        let mut external = original.clone();
        external.push(b' ');
        let expected = if changed_source {
            external.clone()
        } else {
            original
        };
        let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
        let mut session: Session = begin(&runtime, targets.session_targets());
        let snap = session.snapshot();
        let (result, c) = with_canonical_prepare_hooks(
            move |point, _| {
                if changed_source && point == PrepareFailPoint::BeforeOriginalRead {
                    fs::write(&target, &external)?;
                }
                if !changed_source && point == PrepareFailPoint::StagedWrite {
                    return Err(io::Error::new(io::ErrorKind::PermissionDenied, CANARY));
                }
                Ok(())
            },
            || update_template(&mut runtime, &mut session, context(&snap), &input).unwrap(),
        );
        assert_eq!((c.calls, c.allocations), (1, 1));
        assert_eq!(
            result.execution.diagnostic().disk,
            DiskState::NotApplied,
            "{result:?}"
        );
        assert!(result.candidate().unwrap().name() == CANARY.repeat(2));
        assert!(fs::read(path(&fixture, id)).unwrap() == expected);
        assert_eq!(
            match &input.intent {
                TemplateEditIntent::SetName(s) => s.as_ptr(),
                _ => unreachable!(),
            },
            ptr
        );
        let diagnostic = result.execution.diagnostic();
        assert!(diagnostic.artifact_write.is_some());
        for printed in [
            format!("{result:?}"),
            format!("{input:?}"),
            format!("{diagnostic:?}"),
        ] {
            assert!(!printed.contains(CANARY));
        }
        if diagnostic.recovery_required {
            runtime.recover().unwrap();
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn g6_update_revision_overflow_and_deleted_source_reuse_domain_contract() {
    for deleted in [false, true] {
        let fixture = Fixture::new();
        let old = defined();
        let id = old.template_id();
        let mut value: serde_json::Value =
            serde_json::from_slice(&artifact::encode_template(&old).unwrap()).unwrap();
        if deleted {
            value["lifecycle"] = "deleted".into();
        } else {
            value["revision"] = u32::MAX.into();
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        let old = artifact::decode_template(&bytes).unwrap();
        install(&fixture, &old);
        let mut runtime = fixture.runtime();
        let original = fs::read(path(&fixture, id)).unwrap();
        let input = UpdateTemplateInput {
            source: source(&mut runtime, id),
            timestamp_utc: LATER.into(),
            intent: TemplateEditIntent::SetName("different".into()),
        };
        let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
        let mut session: Session = begin(&runtime, targets.session_targets());
        let snap = session.snapshot();
        let (result, c, k) = observe(|| {
            update_template(&mut runtime, &mut session, context(&snap), &input).unwrap()
        });
        assert_no_io(&result.execution, c, k);
        let Some(BodyOutcome::Rejected(e)) = result.execution.body() else {
            panic!()
        };
        let Some(TemplateUseCaseError::Mutation(e)) = e.domain_cause() else {
            panic!()
        };
        assert_eq!(
            e.category(),
            if deleted {
                TemplateMutationErrorCategory::TemplateIsTombstoned
            } else {
                TemplateMutationErrorCategory::RevisionOverflow
            }
        );
        assert!(fs::read(path(&fixture, id)).unwrap() == original);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
