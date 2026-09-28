use super::*;
use crate::data::application::documents::persistence::*;
use crate::data::{
    artifact::{
        DocumentEdit, DocumentEditSet, DocumentMaterializationOutcomeKind,
        DocumentSaveErrorCategory, DocumentSaveOutcome, DocumentSaveOutcomeKind, DocumentValueEdit,
        OptionId, TemplateRevision,
    },
    json::parse_strict_lossless_json_object,
    repository::{test_support as repo_hooks, RepositoryStage},
};
use serde_json::{json, Value};

mod boundaries;
mod policies;
mod scale;
mod transactions;

use std::sync::atomic::{AtomicUsize, Ordering};

// 편집 payload에는 Clone/Debug/Serialize가 없다. G8 실행 이후에도 caller가 drop을 결정한다.
struct Payload(Box<[u8]>, Arc<AtomicUsize>);
impl Payload {
    fn new() -> Self {
        Self(
            PRIVATE.as_bytes().to_vec().into_boxed_slice(),
            Arc::new(AtomicUsize::new(0)),
        )
    }
}
impl Drop for Payload {
    fn drop(&mut self) {
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}
struct RequestProof {
    edits: DocumentEditSet,
    edits_ptr: *const DocumentEditSet,
    timestamp: String,
    timestamp_ptr: *const u8,
    document: DocumentSource,
    template: TemplateSource,
    payload: *const u8,
    drops: Arc<AtomicUsize>,
}
impl RequestProof {
    fn capture(input: &SaveDocumentInput, payload: &Payload) -> Self {
        Self {
            edits: input.edits.clone(),
            edits_ptr: &input.edits,
            timestamp: input.timestamp_utc.clone(),
            timestamp_ptr: input.timestamp_utc.as_ptr(),
            document: DocumentSource {
                id: input.document.id,
                token: input.document.token.clone(),
            },
            template: TemplateSource {
                id: input.template.id,
                token: input.template.token.clone(),
                expected_revision: input.template.expected_revision,
            },
            payload: payload.0.as_ptr(),
            drops: Arc::clone(&payload.1),
        }
    }
    fn assert(&self, input: &SaveDocumentInput, payload: &Payload) {
        assert!(
            input.edits == self.edits && std::ptr::eq(&input.edits, self.edits_ptr),
            "typed edits owner preserved"
        );
        assert!(
            input.timestamp_utc == self.timestamp
                && input.timestamp_utc.as_ptr() == self.timestamp_ptr
        );
        assert!(
            input.document.id == self.document.id && input.document.token == self.document.token
        );
        assert!(
            input.template.id == self.template.id && input.template.token == self.template.token
        );
        assert_eq!(
            input.template.expected_revision,
            self.template.expected_revision
        );
        assert!(payload.0.as_ptr() == self.payload && payload.0.as_ref() == PRIVATE.as_bytes());
        assert_eq!(self.drops.load(Ordering::SeqCst), 0);
    }
    fn drop_payload(&self, payload: Payload) {
        drop(payload);
        assert_eq!(self.drops.load(Ordering::SeqCst), 1);
    }
}

const SAVE_TIME: &str = "2026-09-10T00:00:00.000Z";
const AGAIN: &str = "2026-09-11T00:00:00.000Z";
const PRIVATE: &str = "G8 private document payload";
const LEXEMES: &str = r#"{"upper":1E100,"lower":1e100,"zero":-0,"fraction":0.12345678901234567890123456789,"nested":[2,-0]}"#;

fn key(n: u32) -> String {
    field(n).to_string()
}
fn option(n: u32) -> OptionId {
    key(n).parse().unwrap()
}
fn revision(n: u32) -> TemplateRevision {
    TemplateRevision::try_from(n).unwrap()
}
fn raw_bytes(raw: &Value) -> Vec<u8> {
    serde_json::to_string(raw)
        .unwrap()
        .replace("\"__numbers__\"", LEXEMES)
        .into_bytes()
}
fn rich() -> Value {
    json!({"kind":"richText","future":"__numbers__","document":{
        "schemaVersion":1,"future":"__numbers__","content":{"kind":"root",
        "children":[{"kind":"paragraph","children":[{"kind":"text","text":PRIVATE,"marks":["bold"]}]}]}}})
}
fn fixture_raw() -> (Value, Value) {
    let template = json!({"schemaVersion":1,"artifactType":"template","templateId":key(100),
        "revision":3,"name":PRIVATE,"lifecycle":"active","fieldOrder":[key(1),key(2)],"fields":{
        (key(1)):{"label":"current choice","kind":"singleChoice","lifecycle":"active","required":false,
            "introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},
            "presentation":{},"configuration":{"kind":"singleChoice","optionOrder":[key(12)],
            "options":{(key(11)):{"label":"current archived","lifecycle":"archived"},
                (key(12)):{"label":"current active","lifecycle":"active"}}}},
        (key(2)):{"label":"rich","kind":"richText","lifecycle":"active","required":false,
            "introducedRevision":1,"defaultValue":{"kind":"unset"},"initialDefaultValue":{"kind":"unset"},
            "presentation":{},"configuration":{"kind":"richText"}}},
        "presentation":{},"createdAtUtc":TIME,"updatedAtUtc":TIME});
    let document = json!({"schemaVersion":1,"artifactType":"document","documentId":key(200),
        "templateId":key(100),"templateRevision":3,"name":PRIVATE,"future":"__numbers__",
        "fieldValues":{(key(1)):{"kind":"singleChoice","optionId":key(11),"future":"__numbers__"},
            (key(2)):rich()},
        "orphanedFieldDefinitions":{(key(1)):{"label":"historical field","kind":"singleChoice",
            "future":"__numbers__","options":{(key(11)):{"label":"historical option","future":"__numbers__"}}}},
        "createdAtUtc":TIME,"updatedAtUtc":TIME});
    (template, document)
}
fn seed_raw(f: &Fixture, template: &Value, document: &Value) -> (TemplateId, DocumentId) {
    let tb = raw_bytes(template);
    let db = raw_bytes(document);
    let t = artifact::decode_template(&tb).unwrap();
    let d = artifact::decode_document(&db).unwrap();
    fs::create_dir_all(f.root.join("templates")).unwrap();
    fs::create_dir_all(f.root.join("documents")).unwrap();
    fs::write(template_path(f, t.template_id()), tb).unwrap();
    fs::write(document_path(f, d.document_id()), db).unwrap();
    (t.template_id(), d.document_id())
}
fn seed(f: &Fixture) -> (TemplateId, DocumentId) {
    let (t, d) = fixture_raw();
    assert_numbers(&raw_bytes(&d));
    seed_raw(f, &t, &d)
}
fn doc_source(rt: &mut ProjectRuntime, id: DocumentId) -> DocumentSource {
    let ready = rt.ready().unwrap();
    let repository = ArtifactRepository::new(&ready).unwrap();
    let loaded = repository.load_document(id).unwrap();
    DocumentSource {
        id,
        token: loaded.source().clone(),
    }
}
fn save_input(
    rt: &mut ProjectRuntime,
    tid: TemplateId,
    did: DocumentId,
    edits: Vec<DocumentEdit>,
) -> SaveDocumentInput {
    SaveDocumentInput {
        document: doc_source(rt, did),
        template: source(rt, tid),
        edits: DocumentEditSet::new(edits),
        timestamp_utc: SAVE_TIME.into(),
    }
}
fn mat_input(
    rt: &mut ProjectRuntime,
    tid: TemplateId,
    did: DocumentId,
) -> MaterializeDocumentInput {
    MaterializeDocumentInput {
        document: doc_source(rt, did),
        template: source(rt, tid),
        timestamp_utc: SAVE_TIME.into(),
    }
}
fn doc_targets(did: DocumentId) -> ExactWriteTargets {
    ExactWriteTargets::new([ArtifactSourceId::Document(did)]).unwrap()
}
fn disk(path: &std::path::Path) -> (Vec<u8>, std::time::SystemTime) {
    (
        fs::read(path).unwrap(),
        fs::metadata(path).unwrap().modified().unwrap(),
    )
}
fn assert_disk(path: &std::path::Path, before: &(Vec<u8>, std::time::SystemTime)) {
    assert!(disk(path) == *before, "artifact bytes and mtime preserved");
}
fn assert_numbers(bytes: &[u8]) {
    let tree = parse_strict_lossless_json_object(bytes).unwrap();
    let expected = parse_strict_lossless_json_object(LEXEMES.as_bytes()).unwrap();
    for path in [
        vec!["future".into()],
        vec!["fieldValues".into(), key(1), "future".into()],
        vec!["fieldValues".into(), key(2), "future".into()],
        vec!["orphanedFieldDefinitions".into(), key(1), "future".into()],
        vec![
            "orphanedFieldDefinitions".into(),
            key(1),
            "options".into(),
            key(11),
            "future".into(),
        ],
    ] {
        let refs: Vec<_> = path.iter().map(String::as_str).collect();
        assert!(
            tree.object_path(&refs)
                .expect("fixed metadata owner exists")
                == &expected,
            "fixed owner numeric lexemes"
        );
    }
    let expected_rich = parse_strict_lossless_json_object(&raw_bytes(&rich())).unwrap();
    assert!(
        tree.object_path(&["fieldValues", &key(2)])
            .expect("rich owner exists")
            == &expected_rich,
        "untouched rich-text entire subtree"
    );
    assert_owner(
        bytes,
        &["fieldValues", &key(1)],
        &json!({"kind":"singleChoice","optionId":key(11),"future":"__numbers__"}),
    );
    assert_owner(
        bytes,
        &["orphanedFieldDefinitions", &key(1)],
        &json!({"kind":"singleChoice","label":"historical field","future":"__numbers__",
        "options":{(key(11)):{"label":"historical option","future":"__numbers__"}}}),
    );
    assert_owner(
        bytes,
        &["orphanedFieldDefinitions", &key(1), "options", &key(11)],
        &json!({"label":"historical option","future":"__numbers__"}),
    );
}
fn assert_owner(bytes: &[u8], path: &[&str], expected: &Value) {
    let tree = parse_strict_lossless_json_object(bytes).unwrap();
    let expected = parse_strict_lossless_json_object(&raw_bytes(expected)).unwrap();
    assert!(
        tree.object_path(path)
            .expect("intended owner subtree exists")
            == &expected,
        "independent expected owner subtree"
    );
}
fn assert_warning(warnings: &artifact::DocumentMaterializationWarnings) {
    use artifact::DocumentReconciliationWarningCategory;
    assert_eq!(warnings.len(), 1);
    let w = &warnings.as_slice()[0];
    assert_eq!(
        w.category(),
        DocumentReconciliationWarningCategory::ArchivedOptionSelected
    );
    assert_eq!(w.field_id(), field(1));
    assert_eq!(w.count(), 1);
    assert!(!w.truncated());
}
fn no_io(c: PrepareCounts, k: usize) {
    assert_eq!((c.calls, c.allocations, k), (0, 0, 0));
}
fn mutate(rt: &mut ProjectRuntime, tid: TemplateId, intent: TemplateEditIntent) {
    let input = UpdateTemplateInput {
        source: source(rt, tid),
        timestamp_utc: LATER.into(),
        intent,
    };
    let targets = ExactWriteTargets::new([ArtifactSourceId::Template(tid)]).unwrap();
    let mut session: Session = begin(rt, targets.session_targets());
    let snap = session.snapshot();
    let result = update_template(rt, &mut session, context(&snap), &input).unwrap();
    assert_eq!(
        result.execution.diagnostic().disk,
        DiskState::Committed,
        "{result:?}"
    );
    session.end_edit().unwrap();
}
fn historical_field(rt: &mut ProjectRuntime, tid: TemplateId, required: bool) {
    add_field(
        rt,
        tid,
        NewFieldDraft::new(
            field(3),
            "new field".into(),
            FieldKind::SingleLineText,
            NewFieldConfiguration::single_line_text(),
            false,
            None,
            if required {
                FieldValueDraft::unset()
            } else {
                FieldValueDraft::single_line_text("initial".into())
            },
        ),
        LATER,
    );
    if required {
        mutate(
            rt,
            tid,
            TemplateEditIntent::SetFieldRequired {
                field: field(3),
                required: true,
            },
        );
    }
}

#[test]
fn g8_historical_required_edit_saves_original_and_refreshed_empty_save_is_no_write() {
    let f = Fixture::new();
    let (tid, did) = seed(&f);
    let mut rt = f.runtime();
    historical_field(&mut rt, tid, true);
    let input = save_input(
        &mut rt,
        tid,
        did,
        vec![DocumentEdit::SetValue(
            field(3),
            DocumentValueEdit::single_line_text("filled".into()),
        )],
    );
    assert_eq!(input.template.expected_revision, revision(5));
    let mat = mat_input(&mut rt, tid, did);
    let original = disk(&document_path(&f, did));
    let template_before = disk(&template_path(&f, tid));
    assert_numbers(&original.0);
    let input_edits = input.edits.clone();
    let edits_ptr = &input.edits as *const _;
    let targets = doc_targets(did);
    let mut session: Session = begin(&rt, targets.session_targets());
    let snap = session.snapshot();
    // 같은 실제 before-state에서 no-edit materialize는 required unset을 해결하지 못한다.
    let (blocked, c, k) =
        observe(|| materialize_document(&mut rt, &mut session, context(&snap), &mat).unwrap());
    no_io(c, k);
    assert!(blocked.outcome().is_none());
    let Some(BodyOutcome::Rejected(error)) = blocked.execution.body() else {
        panic!("materialization rejection")
    };
    assert!(
        matches!(error.domain_cause(), Some(DocumentUpdateError::Materialization(e))
        if e.category() == artifact::DocumentMaterializationErrorCategory::BlockingIssues)
    );
    assert_disk(&document_path(&f, did), &original);
    let ((result, c, k), hooks) = repo_hooks::scoped(
        Some((
            RepositoryStage::Namespace,
            0,
            Box::new(|_| panic!("Replace does not create namespaces")),
        )),
        false,
        || observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap()),
    );
    assert_eq!(hooks.hooks, 0);
    assert_eq!(hooks.decoded, 2);
    assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
    assert_eq!(
        result.execution.diagnostic().disk,
        DiskState::Committed,
        "{result:?}"
    );
    let owner = result.outcome().unwrap();
    assert_eq!(owner.kind(), DocumentSaveOutcomeKind::Changed);
    assert_warning(owner.warnings());
    assert_eq!(owner.document().template_revision(), revision(5));
    assert!(owner.document().updated_at_utc() == SAVE_TIME);
    let bytes = artifact::encode_document(owner.document()).unwrap();
    assert_numbers(&bytes);
    let saved: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        saved["fieldValues"][key(3)]["value"] == "filled",
        "typed edit fulfilled historical required field"
    );
    assert!(
        fs::read(document_path(&f, did)).unwrap() == bytes,
        "committed candidate bytes"
    );
    assert!(
        input.edits == input_edits && std::ptr::eq(&input.edits, edits_ptr),
        "borrowed edits unchanged"
    );
    assert_disk(&template_path(&f, tid), &template_before);
    session.end_edit().unwrap();
    rt.close().unwrap();
    let mut rt = f.runtime();
    let mut fresh = save_input(&mut rt, tid, did, vec![]);
    fresh.timestamp_utc = AGAIN.into();
    let before = disk(&document_path(&f, did));
    let mut session: Session = begin(&rt, targets.session_targets());
    let snap = session.snapshot();
    let (unchanged, c, k) =
        observe(|| save_document(&mut rt, &mut session, context(&snap), &fresh).unwrap());
    no_io(c, k);
    assert!(matches!(
        unchanged.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    let outcome = unchanged.outcome().unwrap();
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert_warning(outcome.warnings());
    assert!(outcome.document().updated_at_utc() == SAVE_TIME);
    assert_disk(&document_path(&f, did), &before);
    assert_disk(&template_path(&f, tid), &template_before);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g8_materialize_initial_default_then_refreshed_no_write_preserves_warning_and_native_bytes() {
    let f = Fixture::new();
    let (tid, did) = seed(&f);
    let mut rt = f.runtime();
    historical_field(&mut rt, tid, false);
    let targets = doc_targets(did);
    let mut session: Session = begin(&rt, targets.session_targets());
    let snap = session.snapshot();
    let input = mat_input(&mut rt, tid, did);
    let original = disk(&document_path(&f, did));
    let template_before = disk(&template_path(&f, tid));
    let original_document = artifact::decode_document(&original.0).unwrap();
    assert_eq!(original_document.template_revision(), revision(3));
    assert!(original_document.updated_at_utc() == TIME);
    assert!(!original_document.field_values().contains_key(&field(3)));
    assert_numbers(&original.0);
    assert_eq!(input.template.expected_revision, revision(4));
    assert_owner(
        &template_before.0,
        &["fields", &key(3), "initialDefaultValue"],
        &json!({"kind":"text","value":"initial"}),
    );
    let assert_materialized = |bytes: &[u8]| {
        let document = artifact::decode_document(bytes).unwrap();
        assert_eq!(document.document_id(), did);
        assert_eq!(document.template_id(), tid);
        assert_eq!(document.template_revision(), revision(4));
        assert!(document.created_at_utc() == TIME && document.updated_at_utc() == SAVE_TIME);
        assert_numbers(bytes);
        assert_owner(
            bytes,
            &["fieldValues", &key(3)],
            &json!({"kind":"text","value":"initial"}),
        );
    };
    let (result, c, k) =
        observe(|| materialize_document(&mut rt, &mut session, context(&snap), &input).unwrap());
    assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
    assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
    let outcome = result.outcome().unwrap();
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    assert_warning(outcome.warnings());
    let bytes = artifact::encode_document(outcome.document().unwrap()).unwrap();
    assert_materialized(&bytes);
    assert!(
        bytes != original.0,
        "materialized candidate differs from original"
    );

    // formatting fixture를 쓰기 전에 application이 실제 저장한 bytes를 직접 검증한다.
    let committed = disk(&document_path(&f, did));
    assert!(
        committed.0 == bytes,
        "materialize committed bytes match candidate before fixture rewrite"
    );
    assert_materialized(&committed.0);
    assert_disk(&template_path(&f, tid), &template_before);
    session.end_edit().unwrap();
    rt.close().unwrap();

    // runtime/lock을 새로 열고, 수동 재작성하지 않은 실제 저장 파일에서 source를 발급받는다.
    let mut rt = f.runtime();
    assert_disk(&document_path(&f, did), &committed);
    let fresh = {
        let ready = rt.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let document = repository.load_document(did).unwrap();
        let template = repository.load_template(tid).unwrap();
        let reopened = artifact::encode_document(document.artifact()).unwrap();
        assert!(
            reopened == committed.0,
            "reopened document matches actual committed bytes"
        );
        assert_materialized(&reopened);
        assert_eq!(template.artifact().revision(), revision(4));
        assert!(template.source() == &input.template.token);
        assert!(document.source() != &input.document.token);
        MaterializeDocumentInput {
            document: DocumentSource {
                id: did,
                token: document.source().clone(),
            },
            template: TemplateSource {
                id: tid,
                token: template.source().clone(),
                expected_revision: template.artifact().revision(),
            },
            timestamp_utc: AGAIN.into(),
        }
    };
    let before_no_write = disk(&document_path(&f, did));
    let mut session: Session = begin(&rt, targets.session_targets());
    let snap = session.snapshot();
    let ((result, c, k), hooks) = repo_hooks::scoped(
        Some((
            RepositoryStage::Namespace,
            0,
            Box::new(|_| panic!("NoWrite must not create namespace")),
        )),
        false,
        || observe(|| materialize_document(&mut rt, &mut session, context(&snap), &fresh).unwrap()),
    );
    no_io(c, k);
    assert_eq!((hooks.hooks, hooks.decoded), (0, 2));
    assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
    assert!(matches!(
        result.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    let outcome = result.outcome().unwrap();
    assert_eq!(
        outcome.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert!(outcome.document().is_none());
    assert_warning(outcome.warnings());
    assert_disk(&document_path(&f, did), &before_no_write);
    assert_materialized(&fs::read(document_path(&f, did)).unwrap());
    assert_disk(&template_path(&f, tid), &template_before);

    // 실제 저장·재열기·첫 NoWrite 확인을 마친 뒤 별도의 formatting-only control을 시작한다.
    let mut native = committed.0.clone();
    native.extend_from_slice(b"\n  \r\n");
    fs::write(document_path(&f, did), &native).unwrap();
    let before = disk(&document_path(&f, did));
    assert_materialized(&before.0);
    let mut formatted = mat_input(&mut rt, tid, did);
    assert!(formatted.document.token != fresh.document.token);
    formatted.timestamp_utc = AGAIN.into();
    let ((result, c, k), hooks) = repo_hooks::scoped(
        Some((
            RepositoryStage::Namespace,
            0,
            Box::new(|_| panic!("NoWrite must not create namespace")),
        )),
        false,
        || {
            observe(|| {
                materialize_document(&mut rt, &mut session, context(&snap), &formatted).unwrap()
            })
        },
    );
    no_io(c, k);
    assert_eq!((hooks.hooks, hooks.decoded), (0, 2));
    assert_eq!(result.execution.diagnostic().disk, DiskState::NoWrite);
    assert!(matches!(
        result.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    let outcome = result.outcome().unwrap();
    assert_eq!(
        outcome.kind(),
        DocumentMaterializationOutcomeKind::Unchanged
    );
    assert!(outcome.document().is_none());
    assert_warning(outcome.warnings());
    assert_disk(&document_path(&f, did), &before);
    assert_materialized(&fs::read(document_path(&f, did)).unwrap());
    assert_disk(&template_path(&f, tid), &template_before);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    session.end_edit().unwrap();
    rt.close().unwrap();
}
