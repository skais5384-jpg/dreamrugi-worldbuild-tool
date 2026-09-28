use super::*;

mod integration;
mod persistence;
use crate::data::{
    application::{
        diagnostics::{ApplicationCategory, ApplicationStage},
        templates::{
            create_template, prepare_create_template, tombstone_template, update_template,
            CreateTemplateInput, TemplateEditIntent, TombstoneTemplateInput, UpdateTemplateInput,
        },
        write::BodyOutcome,
    },
    artifact::{
        template_mutation::{
            FieldValueDraft, NewFieldConfiguration, NewFieldDraft, NewFieldInsertion,
        },
        FieldId, FieldKind, TemplateId,
    },
    collaboration_lock::NoLockService,
    edit_session::EditSessionState,
    project_runtime::RuntimeState,
    repository::ArtifactSourceId,
    transaction::test_support::{
        with_canonical_prepare_hooks, with_commit_io_factory, CommitTestPoint, PrepareCounts,
    },
};
use std::{cell::Cell, fs, path::PathBuf, rc::Rc, sync::Arc};

const TIME: &str = "2026-09-09T05:10:11.012Z";
const LATER: &str = "2026-09-09T06:10:11.012Z";
const CANARY: &str = "G7_private_document C:/Users/hidden/document.json?token=secret";
type Session = EditSessionService<(), ()>;

struct Fixture {
    base: PathBuf,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!("worldbuild-g7-{}", uuid::Uuid::new_v4()));
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        Self { base, root }
    }
    fn runtime(&self) -> ProjectRuntime {
        let mut runtime = ProjectRuntime::acquire(&self.root, &self.base.join("locks")).unwrap();
        runtime.recover().unwrap();
        runtime
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.base) {
            // 원 테스트 실패를 덮지 않고 안전한 cleanup 진단만 추가한다.
            if std::thread::panicking() {
                eprintln!("G7 fixture cleanup failed: {:?}", error.kind());
            } else {
                panic!("G7 fixture cleanup failed: {:?}", error.kind());
            }
        }
    }
}

fn context(snapshot: &crate::data::edit_session::EditSessionSnapshot) -> TemplateWriteContext<'_> {
    TemplateWriteContext {
        project: snapshot.project_fingerprint().unwrap(),
        session: snapshot.session_id().unwrap(),
    }
}
fn begin<P>(
    runtime: &ProjectRuntime,
    targets: &[ProjectRelativePath],
) -> EditSessionService<P, ()> {
    let mut session = EditSessionService::new(Arc::new(NoLockService::new()));
    session
        .begin_edit(runtime.project_fingerprint(), targets.to_vec())
        .unwrap();
    session
}
fn source(runtime: &mut ProjectRuntime, id: TemplateId) -> TemplateSource {
    let ready = runtime.ready().unwrap();
    let repository = ArtifactRepository::new(&ready).unwrap();
    let loaded = repository.load_template(id).unwrap();
    TemplateSource {
        id,
        token: loaded.source().clone(),
        expected_revision: loaded.artifact().revision(),
    }
}
fn template_path(fixture: &Fixture, id: TemplateId) -> PathBuf {
    fixture
        .root
        .join(ArtifactSourceId::Template(id).path().unwrap().as_str())
}
fn document_path(fixture: &Fixture, id: DocumentId) -> PathBuf {
    fixture
        .root
        .join(ArtifactSourceId::Document(id).path().unwrap().as_str())
}
fn seed_template(runtime: &mut ProjectRuntime) -> TemplateId {
    let input = CreateTemplateInput {
        name: "G7 template".into(),
        presentation_token: None,
        timestamp_utc: TIME.into(),
    };
    let mut prepared = prepare_create_template(runtime, &input).unwrap();
    let id = prepared.template_id();
    let mut session: Session = begin(runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    let execution = create_template(runtime, &mut session, context(&snapshot), &mut prepared);
    assert_eq!(
        execution.diagnostic().disk,
        DiskState::Committed,
        "{execution:?}"
    );
    session.end_edit().unwrap();
    id
}
fn field(n: u32) -> FieldId {
    format!("77777777-7777-4777-8777-{n:012x}").parse().unwrap()
}
fn add_field(runtime: &mut ProjectRuntime, id: TemplateId, draft: NewFieldDraft, time: &str) {
    let input = UpdateTemplateInput {
        source: source(runtime, id),
        timestamp_utc: time.into(),
        intent: TemplateEditIntent::CreateField {
            draft,
            insertion: NewFieldInsertion::Append,
        },
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(runtime, targets.session_targets());
    let snapshot = session.snapshot();
    assert_eq!(
        update_template(runtime, &mut session, context(&snapshot), &input)
            .unwrap()
            .execution
            .diagnostic()
            .disk,
        DiskState::Committed
    );
    session.end_edit().unwrap();
}
fn seed_defaults_and_archived_template(runtime: &mut ProjectRuntime) -> TemplateId {
    let id = seed_template(runtime);
    add_field(
        runtime,
        id,
        NewFieldDraft::new(
            field(1),
            "optional legacy current".into(),
            FieldKind::SingleLineText,
            NewFieldConfiguration::single_line_text(),
            false,
            None,
            FieldValueDraft::single_line_text("initial default".into()),
        ),
        LATER,
    );
    let current = UpdateTemplateInput {
        source: source(runtime, id),
        timestamp_utc: "2026-09-09T07:10:11.012Z".into(),
        intent: TemplateEditIntent::SetCurrentDefault {
            field: field(1),
            value: FieldValueDraft::single_line_text("current default".into()),
        },
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(runtime, targets.session_targets());
    let snapshot = session.snapshot();
    assert_eq!(
        update_template(runtime, &mut session, context(&snapshot), &current)
            .unwrap()
            .execution
            .diagnostic()
            .disk,
        DiskState::Committed
    );
    session.end_edit().unwrap();
    add_field(
        runtime,
        id,
        NewFieldDraft::new(
            field(2),
            "archived".into(),
            FieldKind::SingleLineText,
            NewFieldConfiguration::single_line_text(),
            false,
            None,
            FieldValueDraft::single_line_text("archived default".into()),
        ),
        "2026-09-09T08:10:11.012Z",
    );
    let archived = UpdateTemplateInput {
        source: source(runtime, id),
        timestamp_utc: "2026-09-09T09:10:11.012Z".into(),
        intent: TemplateEditIntent::ArchiveField(field(2)),
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(id)]).unwrap();
    let mut session: Session = begin(runtime, targets.session_targets());
    let snapshot = session.snapshot();
    assert_eq!(
        update_template(runtime, &mut session, context(&snapshot), &archived)
            .unwrap()
            .execution
            .diagnostic()
            .disk,
        DiskState::Committed
    );
    session.end_edit().unwrap();
    id
}
fn observe<T>(body: impl FnOnce() -> T) -> (T, PrepareCounts, usize) {
    let commits = Rc::new(Cell::new(0));
    let count = Rc::clone(&commits);
    let (value, prepare) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        count.set(count.get() + 1);
                    }
                    Ok(())
                },
                body,
            )
        },
    );
    (value, prepare, commits.get())
}
fn assert_no_io(
    result: &WriteExecution<(), DocumentUseCaseError>,
    counts: PrepareCounts,
    commits: usize,
) {
    assert_eq!(
        (counts.calls, counts.allocations, commits),
        (0, 0, 0),
        "{result:?}"
    );
}

#[test]
fn g7_creates_document_from_actual_template_without_existing_documents_namespace() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let template = seed_template(&mut runtime);
    let original = fs::read(template_path(&fixture, template)).unwrap();
    let mtime = fs::metadata(template_path(&fixture, template))
        .unwrap()
        .modified()
        .unwrap();
    assert!(!fixture.root.join("documents").exists());
    let input = CreateDocumentInput {
        source: source(&mut runtime, template),
        name: CANARY.into(),
        timestamp_utc: LATER.into(),
    };
    let mut prepared = prepare_create_document(&mut runtime, &input).unwrap();
    let id = prepared.document_id();
    let mut session: Session = begin(&runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    let (result, counts, commits) = observe(|| {
        create_document_from_template(
            &mut runtime,
            &mut session,
            context(&snapshot),
            &mut prepared,
        )
    });
    assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
    assert_eq!((counts.calls, counts.allocations, commits), (1, 1, 1));
    assert_eq!(prepared.state(), CreationState::Committed);
    assert!(document_path(&fixture, id).exists());
    assert!(fs::read(template_path(&fixture, template)).unwrap() == original);
    assert_eq!(
        fs::metadata(template_path(&fixture, template))
            .unwrap()
            .modified()
            .unwrap(),
        mtime
    );
    let ready = runtime.ready().unwrap();
    let repository = ArtifactRepository::new(&ready).unwrap();
    let document = repository.load_document(id).unwrap();
    assert_eq!(document.artifact().document_id(), id);
    assert_eq!(document.artifact().template_id(), template);
    assert_eq!(
        document.artifact().template_revision(),
        input.source.expected_revision
    );
    assert!(document.artifact().name() == CANARY);
    assert_eq!(document.artifact().created_at_utc(), LATER);
    assert_eq!(document.artifact().updated_at_utc(), LATER);
    drop(repository);
    drop(ready);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g7_creates_unset_values_and_archived_snapshot_without_legacy_defaults() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let template = seed_defaults_and_archived_template(&mut runtime);
    let input = CreateDocumentInput {
        source: source(&mut runtime, template),
        name: CANARY.into(),
        timestamp_utc: "2026-09-09T10:10:11.012Z".into(),
    };
    let mut prepared = prepare_create_document(&mut runtime, &input).unwrap();
    let id = prepared.document_id();
    let mut session: Session = begin(&runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    assert_eq!(
        create_document_from_template(
            &mut runtime,
            &mut session,
            context(&snapshot),
            &mut prepared
        )
        .diagnostic()
        .disk,
        DiskState::Committed
    );
    let ready = runtime.ready().unwrap();
    let repository = ArtifactRepository::new(&ready).unwrap();
    let document = repository.load_document(id).unwrap();
    assert!(document.artifact().field_values()[&field(1)].is_unset());
    assert!(document.artifact().field_values()[&field(2)].is_unset());
    assert!(document
        .artifact()
        .orphaned_field_definitions()
        .contains_key(&field(2)));
    drop(repository);
    drop(ready);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g7_stale_template_source_is_rejected_before_document_namespace_or_transaction() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let template = seed_template(&mut runtime);
    let input = CreateDocumentInput {
        source: source(&mut runtime, template),
        name: CANARY.into(),
        timestamp_utc: LATER.into(),
    };
    let mut prepared = prepare_create_document(&mut runtime, &input).unwrap();
    let payload = integration::CallerPayload::new(76);
    let owners = integration::OwnerProof::capture(&input, &prepared, &payload);
    let id = prepared.document_id();
    let mut raw = fs::read(template_path(&fixture, template)).unwrap();
    raw.push(b' ');
    fs::write(template_path(&fixture, template), &raw).unwrap();
    let mut session = begin::<integration::CallerPayload>(&runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    let (result, counts, commits) = observe(|| {
        create_document_from_template(
            &mut runtime,
            &mut session,
            context(&snapshot),
            &mut prepared,
        )
    });
    assert_no_io(&result, counts, commits);
    assert_eq!(result.diagnostic().disk, DiskState::NotAttempted);
    assert_eq!(result.diagnostic().stage, ApplicationStage::Build);
    assert_eq!(
        result.diagnostic().category,
        Some(ApplicationCategory::DomainRejected)
    );
    assert!(!fixture.root.join("documents").exists());
    assert!(!document_path(&fixture, id).exists());
    assert_eq!(prepared.state(), CreationState::Uncommitted);
    let Some(BodyOutcome::Rejected(error)) = result.body() else {
        panic!("expected source rejection")
    };
    assert!(matches!(
        error.domain_cause(),
        Some(DocumentUseCaseError::SourceMismatch)
    ));
    assert!(!format!("{prepared:?} {result:?}").contains(CANARY));
    assert!(fs::read(template_path(&fixture, template)).unwrap() == raw);
    owners.assert_preserved(&input, &prepared, &payload);
    owners.assert_caller_drop(payload);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g7_committed_ticket_never_creates_a_second_document() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let template = seed_template(&mut runtime);
    let input = CreateDocumentInput {
        source: source(&mut runtime, template),
        name: CANARY.into(),
        timestamp_utc: LATER.into(),
    };
    let mut prepared = prepare_create_document(&mut runtime, &input).unwrap();
    let id = prepared.document_id();
    let mut session: Session = begin(&runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    assert_eq!(
        create_document_from_template(
            &mut runtime,
            &mut session,
            context(&snapshot),
            &mut prepared
        )
        .diagnostic()
        .disk,
        DiskState::Committed
    );
    let bytes = fs::read(document_path(&fixture, id)).unwrap();
    let (again, counts, commits) = observe(|| {
        create_document_from_template(
            &mut runtime,
            &mut session,
            context(&snapshot),
            &mut prepared,
        )
    });
    assert_no_io(&again, counts, commits);
    assert_eq!(prepared.state(), CreationState::Committed);
    assert!(fs::read(document_path(&fixture, id)).unwrap() == bytes);
    let Some(BodyOutcome::Rejected(error)) = again.body() else {
        panic!("expected completed ticket rejection")
    };
    assert!(matches!(
        error.domain_cause(),
        Some(DocumentUseCaseError::PreparationAlreadyCommitted)
    ));
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g7_tombstone_after_preparation_rejects_document_create_before_namespace() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let template = seed_template(&mut runtime);
    let input = CreateDocumentInput {
        source: source(&mut runtime, template),
        name: CANARY.into(),
        timestamp_utc: LATER.into(),
    };
    let mut prepared = prepare_create_document(&mut runtime, &input).unwrap();
    let document = prepared.document_id();
    let tombstone = TombstoneTemplateInput {
        source: source(&mut runtime, template),
        timestamp_utc: LATER.into(),
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(template)]).unwrap();
    let mut tombstone_session: Session = begin(&runtime, targets.session_targets());
    let tombstone_snapshot = tombstone_session.snapshot();
    let deleted = tombstone_template(
        &mut runtime,
        &mut tombstone_session,
        context(&tombstone_snapshot),
        &tombstone,
    )
    .unwrap();
    assert_eq!(
        deleted.execution.diagnostic().disk,
        DiskState::Committed,
        "{deleted:?}"
    );
    tombstone_session.end_edit().unwrap();
    let mut session: Session = begin(&runtime, prepared.session_targets());
    let snapshot = session.snapshot();
    let (result, counts, commits) = observe(|| {
        create_document_from_template(
            &mut runtime,
            &mut session,
            context(&snapshot),
            &mut prepared,
        )
    });
    assert_no_io(&result, counts, commits);
    assert_eq!(result.diagnostic().disk, DiskState::NotAttempted);
    assert!(!fixture.root.join("documents").exists());
    assert!(!document_path(&fixture, document).exists());
    assert_eq!(prepared.state(), CreationState::Uncommitted);
    assert!(
        matches!(result.body(), Some(BodyOutcome::Rejected(error)) if matches!(error.domain_cause(), Some(DocumentUseCaseError::SourceMismatch)))
    );
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn g7_document_commit_blocks_later_template_tombstone() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let template = seed_template(&mut runtime);
    let input = CreateDocumentInput {
        source: source(&mut runtime, template),
        name: CANARY.into(),
        timestamp_utc: LATER.into(),
    };
    let mut prepared = prepare_create_document(&mut runtime, &input).unwrap();
    let document = prepared.document_id();
    let mut document_session: Session = begin(&runtime, prepared.session_targets());
    let document_snapshot = document_session.snapshot();
    assert_eq!(
        create_document_from_template(
            &mut runtime,
            &mut document_session,
            context(&document_snapshot),
            &mut prepared
        )
        .diagnostic()
        .disk,
        DiskState::Committed
    );
    document_session.end_edit().unwrap();
    let original_template = fs::read(template_path(&fixture, template)).unwrap();
    let original_document = fs::read(document_path(&fixture, document)).unwrap();
    let input = TombstoneTemplateInput {
        source: source(&mut runtime, template),
        timestamp_utc: LATER.into(),
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(template)]).unwrap();
    let mut session: Session = begin(&runtime, targets.session_targets());
    let snapshot = session.snapshot();
    let (result, counts, commits) = observe(|| {
        tombstone_template(&mut runtime, &mut session, context(&snapshot), &input).unwrap()
    });
    assert_eq!((counts.calls, counts.allocations, commits), (0, 0, 0));
    assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
    assert!(fs::read(template_path(&fixture, template)).unwrap() == original_template);
    assert!(fs::read(document_path(&fixture, document)).unwrap() == original_document);
    assert!(matches!(
        result.execution.body(),
        Some(BodyOutcome::Rejected(_))
    ));
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

mod layout;
