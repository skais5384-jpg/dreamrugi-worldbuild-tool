use super::*;
use crate::data::{
    application::{
        diagnostics::{ApplicationCategory, DiskState},
        templates::{update_template, UpdateTemplateInput},
        write::{BodyOutcome, TransactionResult},
    },
    artifact::{
        template_mutation::{
            FieldValueDraft, NewFieldConfiguration, NewFieldDraft, NewFieldInsertion,
        },
        DocumentEdit, DocumentId, DocumentSaveErrorCategory, DocumentSaveStage, DocumentValueEdit,
        FieldId, FieldKind, TemplateId, TemplateRevision,
    },
    collaboration_lock::NoLockService,
    edit_session::{EditSessionSnapshot, EditSessionState},
    json::parse_strict_lossless_json_object,
    project_runtime::RuntimeState,
    repository::{test_support as repo_hooks, ArtifactRepository, RepositoryStage},
    transaction::test_support::{
        with_canonical_prepare_hooks, with_commit_io_factory, CommitTestPoint, PrepareCounts,
    },
};
use serde_json::{json, Value};
use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::SystemTime,
};

const TIME: &str = "2026-09-09T05:10:11.012Z";
const LATER: &str = "2026-09-09T06:10:11.012Z";
const SAVE: &str = "2026-09-10T00:00:00.000Z";
const AGAIN: &str = "2026-09-11T00:00:00.000Z";
const PRIVATE: &str = "G9 private editor payload";
const TOKENS: &str = r#"{"upper":1E100,"lower":1e100,"zero":-0,"fraction":0.12345678901234567890123456789,"nested":[2,-0]}"#;
// G13도 검증된 historical fixture bytes를 재사용하며 artifact 원문을 Value 왕복으로 바꾸지 않는다.
pub(crate) fn guarded_fixture_bytes() -> (Vec<u8>, Vec<u8>) {
    let (template, document) = fixture_raw();
    (raw_bytes(&template), raw_bytes(&document))
}
type Session = EditSessionService<Payload, ()>;

pub(in crate::data::application) fn key(n: u32) -> String {
    format!("99999999-9999-4999-8999-{n:012x}")
}
pub(in crate::data::application) fn field(n: u32) -> FieldId {
    key(n).parse().unwrap()
}
pub(in crate::data::application) fn tid() -> TemplateId {
    key(100).parse().unwrap()
}
pub(in crate::data::application) fn did() -> DocumentId {
    key(200).parse().unwrap()
}
fn sentinel_id() -> DocumentId {
    key(201).parse().unwrap()
}
pub(in crate::data::application) fn revision(n: u32) -> TemplateRevision {
    TemplateRevision::try_from(n).unwrap()
}
fn metadata(owner: &str) -> String {
    format!(r#"{{"owner":"{owner}","lexemes":{TOKENS}}}"#)
}
fn raw_bytes(raw: &Value) -> Vec<u8> {
    let mut text = serde_json::to_string(raw).unwrap();
    for owner in [
        "template",
        "field",
        "configuration",
        "option",
        "default",
        "document",
        "value",
        "snapshot",
        "snapshot_option",
        "rich_outer",
        "rich_inner",
    ] {
        text = text.replace(&format!("\"__{owner}__\""), &metadata(owner));
    }
    text.into_bytes()
}
fn rich() -> Value {
    json!({"kind":"richText","future":"__rich_outer__","document":{
        "schemaVersion":1,"future":"__rich_inner__","content":{"kind":"root","children":[
            {"kind":"paragraph","children":[{"kind":"text","text":PRIVATE,"marks":["bold"]}]}]}}})
}
pub(crate) fn fixture_raw() -> (Value, Value) {
    let template = json!({"schemaVersion":1,"artifactType":"template","templateId":key(100),
        "revision":3,"name":"template","lifecycle":"active","future":"__template__",
        "fieldOrder":[key(1),key(2),key(4)],"fields":{
            (key(1)):{"label":"current field","kind":"singleChoice","lifecycle":"active","required":false,
                "introducedRevision":1,"future":"__field__","defaultValue":{"kind":"unset"},
                "initialDefaultValue":{"kind":"unset"},"presentation":{},
                "configuration":{"kind":"singleChoice","future":"__configuration__","optionOrder":[key(12)],
                    "options":{(key(11)):{"label":"current archived option","lifecycle":"archived","future":"__option__"},
                               (key(12)):{"label":"current active option","lifecycle":"active"}}}},
            (key(2)):{"label":"rich","kind":"richText","lifecycle":"active","required":false,
                "introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},
                "presentation":{},"configuration":{"kind":"richText"}},
            (key(4)):{"label":"default","kind":"singleLineText","lifecycle":"active","required":false,
                "introducedRevision":1,"defaultValue":{"kind":"text","value":"current","future":"__default__"},
                "initialDefaultValue":{"kind":"unset"},"presentation":{},"configuration":{"kind":"singleLineText"}}},
        "presentation":{},"createdAtUtc":TIME,"updatedAtUtc":TIME});
    let document = json!({"schemaVersion":1,"artifactType":"document","documentId":key(200),
        "templateId":key(100),"templateRevision":3,"name":"document","future":"__document__",
        "fieldValues":{(key(1)):{"kind":"singleChoice","optionId":key(11),"future":"__value__"},
            (key(2)):rich(),(key(4)):{"kind":"text","value":"existing"}},
        "orphanedFieldDefinitions":{(key(1)):{"label":"historical field","kind":"singleChoice",
            "future":"__snapshot__","options":{(key(11)):{"label":"historical option","future":"__snapshot_option__"}}}},
        "createdAtUtc":TIME,"updatedAtUtc":TIME});
    (template, document)
}
pub(in crate::data::application) struct Fixture {
    base: PathBuf,
    pub(in crate::data::application) root: PathBuf,
}
impl Fixture {
    pub(in crate::data::application) fn new() -> Self {
        let base = std::env::temp_dir().join(format!("worldbuild-g9-{}", uuid::Uuid::new_v4()));
        let root = base.join("project");
        fs::create_dir_all(&root).unwrap();
        Self { base, root }
    }
    pub(in crate::data::application) fn runtime(&self) -> ProjectRuntime {
        let mut rt = ProjectRuntime::acquire(&self.root, &self.base.join("locks")).unwrap();
        rt.recover().unwrap();
        rt
    }
    pub(in crate::data::application) fn template_path(&self) -> PathBuf {
        self.root
            .join(ArtifactSourceId::Template(tid()).path().unwrap().as_str())
    }
    pub(in crate::data::application) fn document_path(&self) -> PathBuf {
        self.root
            .join(ArtifactSourceId::Document(did()).path().unwrap().as_str())
    }
    fn sentinel_path(&self) -> PathBuf {
        self.root.join(
            ArtifactSourceId::Document(sentinel_id())
                .path()
                .unwrap()
                .as_str(),
        )
    }
    pub(in crate::data::application) fn seed(&self, template: &Value, document: &Value) {
        let tb = raw_bytes(template);
        let db = raw_bytes(document);
        artifact::decode_template(&tb).unwrap();
        artifact::decode_document(&db).unwrap();
        fs::create_dir_all(self.root.join("templates")).unwrap();
        fs::create_dir_all(self.root.join("documents")).unwrap();
        fs::write(self.template_path(), tb).unwrap();
        fs::write(self.document_path(), db).unwrap();
        let mut sentinel = document.clone();
        sentinel["documentId"] = json!(key(201));
        sentinel["name"] = json!("sentinel");
        fs::write(self.sentinel_path(), raw_bytes(&sentinel)).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.base) {
            if std::thread::panicking() {
                eprintln!("G9 fixture cleanup failed: {:?}", error.kind());
            } else {
                panic!("G9 fixture cleanup failed: {:?}", error.kind());
            }
        }
    }
}
pub(in crate::data::application) fn context(
    snapshot: &EditSessionSnapshot,
) -> TemplateWriteContext<'_> {
    TemplateWriteContext {
        project: snapshot.project_fingerprint().unwrap(),
        session: snapshot.session_id().unwrap(),
    }
}
fn begin(rt: &ProjectRuntime, targets: &[ProjectRelativePath]) -> Session {
    let mut session = Session::new(Arc::new(NoLockService::new()));
    session
        .begin_edit(rt.project_fingerprint(), targets.to_vec())
        .unwrap();
    session
}
fn template_source(rt: &mut ProjectRuntime) -> TemplateSource {
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let loaded = repo.load_template(tid()).unwrap();
    TemplateSource {
        id: tid(),
        token: loaded.source().clone(),
        expected_revision: loaded.artifact().revision(),
    }
}
pub(in crate::data::application) fn load_input(
    rt: &mut ProjectRuntime,
    intent: TemplateEditIntent,
    edits: Vec<DocumentEdit>,
) -> CompositeSaveInput {
    let template = template_source(rt);
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let document = repo.load_document(did()).unwrap();
    CompositeSaveInput {
        template,
        document: DocumentSource {
            id: did(),
            token: document.source().clone(),
        },
        intent,
        edits: DocumentEditSet::new(edits),
        timestamp_utc: SAVE.into(),
    }
}
pub(in crate::data::application) fn required_intent() -> TemplateEditIntent {
    TemplateEditIntent::SetFieldRequired {
        field: field(3),
        required: true,
    }
}
pub(in crate::data::application) fn filled_edit() -> DocumentEdit {
    DocumentEdit::SetValue(
        field(3),
        DocumentValueEdit::single_line_text("filled".into()),
    )
}
pub(in crate::data::application) fn add_historical(rt: &mut ProjectRuntime) {
    let input = UpdateTemplateInput {
        source: template_source(rt),
        timestamp_utc: LATER.into(),
        intent: TemplateEditIntent::CreateField {
            draft: NewFieldDraft::new(
                field(3),
                "historical required later".into(),
                FieldKind::SingleLineText,
                NewFieldConfiguration::single_line_text(),
                false,
                None,
                FieldValueDraft::unset(),
            ),
            insertion: NewFieldInsertion::Append,
        },
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(tid())]).unwrap();
    let mut session = begin(rt, targets.session_targets());
    let snap = session.snapshot();
    let result = update_template(rt, &mut session, context(&snap), &input).unwrap();
    assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
    session.end_edit().unwrap();
}
pub(in crate::data::application) fn seeded_historical() -> (Fixture, ProjectRuntime) {
    let f = Fixture::new();
    let (t, d) = fixture_raw();
    f.seed(&t, &d);
    let mut rt = f.runtime();
    add_historical(&mut rt);
    (f, rt)
}
#[derive(Clone)]
pub(in crate::data::application) struct Pair {
    pub(in crate::data::application) template: (Vec<u8>, SystemTime),
    pub(in crate::data::application) document: (Vec<u8>, SystemTime),
    pub(in crate::data::application) sentinel: (Vec<u8>, SystemTime),
}
fn disk(path: &Path) -> (Vec<u8>, SystemTime) {
    (
        fs::read(path).unwrap(),
        fs::metadata(path).unwrap().modified().unwrap(),
    )
}
pub(in crate::data::application) fn pair(f: &Fixture) -> Pair {
    Pair {
        template: disk(&f.template_path()),
        document: disk(&f.document_path()),
        sentinel: disk(&f.sentinel_path()),
    }
}
pub(in crate::data::application) fn same_pair(f: &Fixture, before: &Pair) {
    let after = pair(f);
    assert!(
        after.template == before.template,
        "Template native bytes and mtime preserved"
    );
    assert!(
        after.document == before.document,
        "Document native bytes and mtime preserved"
    );
    assert!(
        after.sentinel == before.sentinel,
        "other Document remains lazy"
    );
}
pub(in crate::data::application) fn same_old_bytes(f: &Fixture, before: &Pair) {
    assert!(
        fs::read(f.template_path()).unwrap() == before.template.0,
        "rolled-back Template old bytes and timestamps"
    );
    assert!(
        fs::read(f.document_path()).unwrap() == before.document.0,
        "rolled-back Document old bytes and timestamps"
    );
    assert!(
        disk(&f.sentinel_path()) == before.sentinel,
        "sentinel unchanged"
    );
}
fn owner(bytes: &[u8], path: &[&str], expected: &Value) {
    let actual = parse_strict_lossless_json_object(bytes).unwrap();
    let expected = parse_strict_lossless_json_object(&raw_bytes(expected)).unwrap();
    assert!(
        actual.object_path(path).expect("fixed owner exists") == &expected,
        "independent lossless owner expected"
    );
}
fn metadata_owner(bytes: &[u8], path: &[&str], tag: &str) {
    let actual = parse_strict_lossless_json_object(bytes).unwrap();
    let expected = parse_strict_lossless_json_object(metadata(tag).as_bytes()).unwrap();
    assert!(
        actual
            .object_path(path)
            .expect("fixed metadata owner exists")
            == &expected,
        "owner and numeric lexemes preserved"
    );
}
pub(in crate::data::application) fn preserved(template: &[u8], document: &[u8]) {
    for (path, tag) in [
        (vec!["future".into()], "template"),
        (vec!["fields".into(), key(1), "future".into()], "field"),
        (
            vec![
                "fields".into(),
                key(1),
                "configuration".into(),
                "future".into(),
            ],
            "configuration",
        ),
        (
            vec![
                "fields".into(),
                key(1),
                "configuration".into(),
                "options".into(),
                key(11),
                "future".into(),
            ],
            "option",
        ),
        (
            vec![
                "fields".into(),
                key(4),
                "defaultValue".into(),
                "future".into(),
            ],
            "default",
        ),
    ] {
        metadata_owner(
            template,
            &path.iter().map(String::as_str).collect::<Vec<_>>(),
            tag,
        );
    }
    metadata_owner(document, &["future"], "document");
    metadata_owner(document, &["fieldValues", &key(1), "future"], "value");
    owner(document, &["fieldValues", &key(2)], &rich());
    owner(
        document,
        &["fieldValues", &key(4)],
        &json!({"kind":"text","value":"existing"}),
    );
    owner(
        document,
        &["orphanedFieldDefinitions", &key(1)],
        &json!({"label":"historical field","kind":"singleChoice",
        "future":"__snapshot__","options":{(key(11)):{"label":"historical option","future":"__snapshot_option__"}}}),
    );
    metadata_owner(
        document,
        &[
            "orphanedFieldDefinitions",
            &key(1),
            "options",
            &key(11),
            "future",
        ],
        "snapshot_option",
    );
}
pub(in crate::data::application) fn warning(outcome: &DocumentSaveOutcome) {
    let w = outcome.warnings();
    assert_eq!(w.len(), 1);
    let w = &w.as_slice()[0];
    assert_eq!(
        w.category(),
        artifact::DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(w.field_id(), field(1));
    assert_eq!(w.count(), 1);
    assert!(!w.truncated());
}
pub(in crate::data::application) fn candidate_pair(
    result: &CompositeSaveExecution,
) -> (Vec<u8>, Vec<u8>) {
    let t = result
        .template_outcome()
        .expect("Template outcome retained")
        .changed()
        .expect("Template candidate retained");
    let d = result
        .document_outcome()
        .expect("Document outcome retained");
    assert_eq!(d.kind(), DocumentSaveOutcomeKind::Changed);
    warning(d);
    let tb = artifact::encode_template(t).unwrap();
    let db = artifact::encode_document(d.document()).unwrap();
    preserved(&tb, &db);
    assert_eq!(t.template_id(), tid());
    assert_eq!(d.document().document_id(), did());
    assert_eq!(d.document().template_id(), tid());
    assert_eq!(t.revision(), revision(5));
    assert_eq!(d.document().template_revision(), revision(5));
    assert!(t.created_at_utc() == TIME && t.updated_at_utc() == SAVE);
    assert!(d.document().created_at_utc() == TIME && d.document().updated_at_utc() == SAVE);
    assert!(t
        .fields()
        .get(&field(3))
        .expect("introduced field exists")
        .required());
    owner(
        &db,
        &["fieldValues", &key(3)],
        &json!({"kind":"text","value":"filled"}),
    );
    assert!(!format!("{result:?}").contains(PRIVATE));
    (tb, db)
}
pub(in crate::data::application) fn observe<T>(
    run: impl FnOnce() -> T,
) -> (T, PrepareCounts, usize) {
    let commits = Rc::new(Cell::new(0));
    let seen = commits.clone();
    let (result, c) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        seen.set(seen.get() + 1);
                    }
                    Ok(())
                },
                run,
            )
        },
    );
    (result, c, commits.get())
}
fn no_io(c: PrepareCounts, k: usize) {
    assert_eq!((c.calls, c.allocations, k), (0, 0, 0));
}
fn domain(result: &CompositeSaveExecution) -> &CompositeSaveError {
    let Some(BodyOutcome::Rejected(error)) = result.execution.body() else {
        panic!("domain rejection expected")
    };
    error.domain_cause().expect("original typed domain cause")
}
struct Payload {
    bytes: Box<[u8]>,
    drops: Arc<AtomicUsize>,
}
impl Payload {
    fn new() -> Self {
        Self {
            bytes: PRIVATE.as_bytes().to_vec().into_boxed_slice(),
            drops: Arc::new(AtomicUsize::new(0)),
        }
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct InputProof {
    edits: DocumentEditSet,
    edits_ptr: *const DocumentEditSet,
    intent_ptr: *const TemplateEditIntent,
    template: TemplateSource,
    document: DocumentSource,
    time: String,
    time_ptr: *const u8,
    payload: *const u8,
    drops: Arc<AtomicUsize>,
}
impl InputProof {
    fn capture(input: &CompositeSaveInput, payload: &Payload) -> Self {
        assert!(
            matches!(&input.intent,TemplateEditIntent::SetFieldRequired{field:id,required:true} if *id==field(3))
        );
        Self {
            edits: input.edits.clone(),
            edits_ptr: &input.edits,
            intent_ptr: &input.intent,
            template: TemplateSource {
                id: input.template.id,
                token: input.template.token.clone(),
                expected_revision: input.template.expected_revision,
            },
            document: DocumentSource {
                id: input.document.id,
                token: input.document.token.clone(),
            },
            time: input.timestamp_utc.clone(),
            time_ptr: input.timestamp_utc.as_ptr(),
            payload: payload.bytes.as_ptr(),
            drops: payload.drops.clone(),
        }
    }
    fn check(&self, input: &CompositeSaveInput, payload: &Payload) {
        assert!(std::ptr::eq(&input.intent, self.intent_ptr));
        assert!(
            matches!(&input.intent,TemplateEditIntent::SetFieldRequired{field:id,required:true} if *id==field(3))
        );
        assert!(
            input.edits == self.edits && std::ptr::eq(&input.edits, self.edits_ptr),
            "original typed edit contents and owner"
        );
        assert!(
            input.template.id == self.template.id
                && input.template.token == self.template.token
                && input.template.expected_revision == self.template.expected_revision
        );
        assert!(
            input.document.id == self.document.id && input.document.token == self.document.token
        );
        assert!(input.timestamp_utc == self.time && input.timestamp_utc.as_ptr() == self.time_ptr);
        assert!(
            payload.bytes.as_ref() == PRIVATE.as_bytes() && payload.bytes.as_ptr() == self.payload,
            "opaque payload bytes and owner"
        );
        assert_eq!(self.drops.load(Ordering::SeqCst), 0);
    }
    fn drop_payload(&self, p: Payload) {
        drop(p);
        assert_eq!(self.drops.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn g9_historical_required_edit_commits_actual_pair_reopens_and_fresh_pair_is_no_write() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let before = pair(&f);
    preserved(&before.template.0, &before.document.0);
    let t = artifact::decode_template(&before.template.0).unwrap();
    let d = artifact::decode_document(&before.document.0).unwrap();
    assert_eq!(t.revision(), revision(4));
    assert_eq!(d.template_revision(), revision(3));
    assert!(!d.field_values().contains_key(&field(3)));
    owner(
        &before.template.0,
        &["fields", &key(3), "initialDefaultValue"],
        &json!({"kind":"unset"}),
    );
    let payload = Payload::new();
    let proof = InputProof::capture(&input, &payload);
    let request = CompositeWriteRequest::new(&input).unwrap();
    assert!(
        request.session_targets()
            == [
                ArtifactSourceId::Document(did()).path().unwrap(),
                ArtifactSourceId::Template(tid()).path().unwrap()
            ]
    );
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
                update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
            })
        },
    );
    assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
    assert_eq!((hooks.hooks, hooks.decoded), (0, 2));
    assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
    let (tb, db) = candidate_pair(&result);
    assert!(
        tb != before.template.0 && db != before.document.0,
        "both candidates differ from original"
    );
    // application 직후 원 파일 두 개를 먼저 읽는다. fixture 재작성은 이 구간에 없다.
    let committed = pair(&f);
    assert!(
        committed.template.0 == tb,
        "actual committed Template bytes match candidate"
    );
    assert!(
        committed.document.0 == db,
        "actual committed Document bytes match candidate"
    );
    assert!(committed.sentinel == before.sentinel);
    preserved(&committed.template.0, &committed.document.0);
    proof.check(&input, &payload);
    proof.drop_payload(payload);
    session.end_edit().unwrap();
    rt.close().unwrap();

    let mut rt = f.runtime();
    same_pair(&f, &committed);
    let mut fresh = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    fresh.timestamp_utc = AGAIN.into();
    assert!(
        fresh.template.token != input.template.token
            && fresh.document.token != input.document.token
    );
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let reopened_t = repo.load_template(tid()).unwrap();
    let reopened_d = repo.load_document(did()).unwrap();
    assert!(artifact::encode_template(reopened_t.artifact()).unwrap() == tb);
    assert!(artifact::encode_document(reopened_d.artifact()).unwrap() == db);
    preserved(
        &artifact::encode_template(reopened_t.artifact()).unwrap(),
        &artifact::encode_document(reopened_d.artifact()).unwrap(),
    );
    let fresh_request = CompositeWriteRequest::new(&fresh).unwrap();
    let mut session = begin(&rt, fresh_request.session_targets());
    let snap = session.snapshot();
    let (result, c, k) = observe(|| {
        update_template_and_save_document(&mut rt, &mut session, context(&snap), &fresh_request)
    });
    no_io(c, k);
    assert!(matches!(
        result.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
    assert!(matches!(
        result.template_outcome(),
        Some(TemplateMutationOutcome::Unchanged)
    ));
    let doc = result.document_outcome().unwrap();
    assert_eq!(doc.kind(), DocumentSaveOutcomeKind::Unchanged);
    warning(doc);
    assert_eq!(doc.document().template_revision(), revision(5));
    assert!(doc.document().updated_at_utc() == SAVE);
    same_pair(&f, &committed);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g9_document_pure_failure_keeps_template_candidate_and_both_original_files() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![]);
    let before = pair(&f);
    preserved(&before.template.0, &before.document.0);
    let payload = Payload::new();
    let proof = InputProof::capture(&input, &payload);
    let request = CompositeWriteRequest::new(&input).unwrap();
    let mut session = begin(&rt, request.session_targets());
    let snap = session.snapshot();
    let (result, c, k) = observe(|| {
        update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
    });
    no_io(c, k);
    assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
    let Some(CompositeSaveError::Document(cause)) = Some(domain(&result)) else {
        panic!("Document pure error")
    };
    assert_eq!(cause.category(), DocumentSaveErrorCategory::BlockingIssues);
    assert_eq!(cause.stage(), DocumentSaveStage::FinalReconciliation);
    let t = result.template_outcome().unwrap().changed().unwrap();
    assert_eq!(t.revision(), revision(5));
    assert!(t.fields().get(&field(3)).unwrap().required());
    assert!(t.updated_at_utc() == SAVE);
    assert!(
        result.document_outcome().is_none(),
        "no invented partial Document candidate"
    );
    preserved(&artifact::encode_template(t).unwrap(), &before.document.0);
    same_pair(&f, &before);
    proof.check(&input, &payload);
    proof.drop_payload(payload);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

mod boundaries;
pub(crate) mod g14;
mod policies;
mod transactions;
