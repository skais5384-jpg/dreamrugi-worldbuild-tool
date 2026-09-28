//! G7 감사에서 확인한 경계를 실제 제품 회귀로 유지한다. G8 저장 API는 호출하지 않는다.
use super::*;
use crate::data::{
    application::write::TransactionResult,
    artifact::{
        DocumentCreationErrorCategory, DocumentEditSet, DocumentMaterializationOutcomeKind,
        DocumentSaveOutcomeKind,
    },
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
        LockProviderInfo, LockService,
    },
    json::parse_strict_lossless_json_object,
    repository::{test_support as repo_hooks, RepositoryStage},
    transaction::{CommitResultState, PrepareFailPoint},
};
use std::{
    cell::RefCell,
    io,
    os::windows::fs::OpenOptionsExt,
    sync::atomic::{AtomicUsize, Ordering},
};

// payload는 G7에 넘겨 소비하는 값이 아니다. caller가 계속 소유하며 !Clone 계약을 유지한다.
pub(super) struct CallerPayload(Box<u32>, Arc<AtomicUsize>);
impl CallerPayload {
    pub(super) fn new(value: u32) -> Self {
        Self(Box::new(value), Arc::new(AtomicUsize::new(0)))
    }
}
impl Drop for CallerPayload {
    fn drop(&mut self) {
        self.1.fetch_add(1, Ordering::SeqCst);
    }
}

pub(super) struct OwnerProof {
    name: String,
    time: String,
    name_ptr: *const u8,
    time_ptr: *const u8,
    token: crate::data::repository::SourceToken,
    source_id: TemplateId,
    revision: artifact::TemplateRevision,
    candidate: *const DocumentArtifact,
    bytes: Vec<u8>,
    id: DocumentId,
    targets: Vec<ProjectRelativePath>,
    targets_ptr: *const ProjectRelativePath,
    payload: *const u32,
    drops: Arc<AtomicUsize>,
}
impl OwnerProof {
    pub(super) fn capture(
        input: &CreateDocumentInput,
        ticket: &PreparedDocumentCreate,
        payload: &CallerPayload,
    ) -> Self {
        Self {
            name: input.name.clone(),
            time: input.timestamp_utc.clone(),
            name_ptr: input.name.as_ptr(),
            time_ptr: input.timestamp_utc.as_ptr(),
            token: input.source.token.clone(),
            source_id: input.source.id,
            revision: input.source.expected_revision,
            candidate: ticket.candidate(),
            bytes: artifact::encode_document(ticket.candidate()).unwrap(),
            id: ticket.document_id(),
            targets: ticket.session_targets().to_vec(),
            targets_ptr: ticket.session_targets().as_ptr(),
            payload: &*payload.0,
            drops: Arc::clone(&payload.1),
        }
    }
    pub(super) fn assert_preserved(
        &self,
        input: &CreateDocumentInput,
        ticket: &PreparedDocumentCreate,
        payload: &CallerPayload,
    ) {
        assert!(input.name == self.name && input.timestamp_utc == self.time);
        assert!(
            input.name.as_ptr() == self.name_ptr && input.timestamp_utc.as_ptr() == self.time_ptr
        );
        assert!(input.source.token == self.token && input.source.id == self.source_id);
        assert_eq!(input.source.expected_revision, self.revision);
        assert!(std::ptr::eq(ticket.candidate(), self.candidate));
        assert!(artifact::encode_document(ticket.candidate()).unwrap() == self.bytes);
        assert_eq!(ticket.document_id(), self.id);
        assert!(
            ticket.session_targets() == self.targets
                && ticket.session_targets().as_ptr() == self.targets_ptr
        );
        assert!(std::ptr::eq(&*payload.0, self.payload));
        assert_eq!(self.drops.load(Ordering::SeqCst), 0);
    }
    pub(super) fn assert_caller_drop(&self, payload: CallerPayload) {
        drop(payload);
        assert_eq!(self.drops.load(Ordering::SeqCst), 1);
    }
}
#[derive(Debug)]
struct OwnedCause(Arc<()>);
impl std::fmt::Display for OwnedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let _ = &self.0;
        f.write_str("G7_FIX_CAUSE")
    }
}
impl std::error::Error for OwnedCause {}
fn fixture_input(runtime: &mut ProjectRuntime, template: TemplateId) -> CreateDocumentInput {
    CreateDocumentInput {
        source: source(runtime, template),
        name: "G7_FIX_SYNTHETIC".into(),
        timestamp_utc: LATER.into(),
    }
}
fn fixture_journals(f: &Fixture) -> usize {
    let p = f.root.join(".worldbuild/transactions");
    if p.exists() {
        fs::read_dir(p).unwrap().count()
    } else {
        0
    }
}
fn fixture_count_docs(f: &Fixture) -> usize {
    fs::read_dir(f.root.join("documents")).unwrap().count()
}
fn fixture_owner(bytes: &[u8], path: &[&str]) -> Vec<u8> {
    // lossless AST의 serializer는 숫자 lexeme를 보존한다. Value/f64 왕복은 사용하지 않는다.
    let lossless = parse_strict_lossless_json_object(bytes).unwrap();
    let owner = lossless
        .object_path(path)
        .expect("expected exact metadata owner");
    serde_json::to_vec(owner).unwrap()
}
fn fixture_seed(f: &Fixture) -> TemplateId {
    let id: TemplateId = "a7700000-0000-4000-8000-000000000001".parse().unwrap();
    let mut fields = serde_json::Map::new();
    for n in 1..=3 {
        fields.insert(field(n).to_string(),serde_json::json!({
            "label":format!("fixture field {n}"),"lifecycle":if n==3 {"archived"} else {"active"},
            "kind":"singleLineText","required":false,"introducedRevision":1,
            "defaultValue":{"kind":"text","value":format!("current-{n}"),"auditOuter":{"number":format!("LEXEME_{n}"),"owner":n}},
            "initialDefaultValue":{"kind":"text","value":format!("initial-{n}")},
            "configuration":{"kind":"singleLineText"},"presentation":{}
        }));
    }
    let raw = serde_json::json!({"artifactType":"template","schemaVersion":1,"templateId":id.to_string(),"revision":11,
        "name":"fixture source","lifecycle":"active","presentation":{},"fields":fields,
        "fieldOrder":[field(2).to_string(),field(1).to_string()],"createdAtUtc":TIME,"updatedAtUtc":TIME});
    let bytes = serde_json::to_string(&raw)
        .unwrap()
        .replace("\"LEXEME_1\"", "-0")
        .replace("\"LEXEME_2\"", "7E+109")
        .replace("\"LEXEME_3\"", "8e-007")
        .into_bytes();
    assert!(artifact::decode_template(&bytes).is_ok());
    fs::create_dir(f.root.join("templates")).unwrap();
    fs::write(template_path(f, id), bytes).unwrap();
    id
}

#[test]
fn g7_lossless_create_reopen_and_pure_idempotence() {
    let f = Fixture::new();
    let tid = fixture_seed(&f);
    let mut rt = f.runtime();
    let original = fs::read(template_path(&f, tid)).unwrap();
    let mtime = fs::metadata(template_path(&f, tid))
        .unwrap()
        .modified()
        .unwrap();
    for (n, lexeme) in [(1, "-0"), (2, "7E+109"), (3, "8e-007")] {
        let owner = fixture_owner(
            &original,
            &[
                "fields",
                &field(n).to_string(),
                "defaultValue",
                "auditOuter",
            ],
        );
        let expected = format!(r#"{{"number":{lexeme},"owner":{n}}}"#).into_bytes();
        assert!(
            owner == expected,
            "source must contain the exact owner and numeric lexeme"
        );
    }
    let decoded = artifact::decode_template(&original).unwrap();
    assert!(decoded.field_order() == [field(2), field(1)]);
    for n in 1..=3 {
        assert!(
            decoded.fields()[&field(n)].default_value().text()
                == Some(format!("current-{n}").as_str())
        );
        assert!(
            decoded.fields()[&field(n)].initial_default_value().text()
                == Some(format!("initial-{n}").as_str())
        );
    }
    assert_eq!(
        decoded.revision(),
        artifact::TemplateRevision::try_from(11).unwrap()
    );
    let input = fixture_input(&mut rt, tid);
    let token = input.source.token.clone();
    let mut ticket = prepare_create_document(&mut rt, &input).unwrap();
    let id = ticket.document_id();
    assert!(ticket.session_targets() == [ArtifactSourceId::Document(id).path().unwrap()]);
    assert!(!f.root.join("documents").exists());
    let mut session: Session = begin(&rt, ticket.session_targets());
    let snap = session.snapshot();
    let (result, c, k) = observe(|| {
        create_document_from_template(&mut rt, &mut session, context(&snap), &mut ticket)
    });
    assert_eq!(result.diagnostic().disk, DiskState::Committed);
    assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
    assert!(fs::read(template_path(&f, tid)).unwrap() == original);
    assert!(
        fs::metadata(template_path(&f, tid))
            .unwrap()
            .modified()
            .unwrap()
            == mtime
    );
    session.end_edit().unwrap();
    rt.close().unwrap();
    let mut rt = f.runtime();
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let source = repo.load_template(tid).unwrap();
    assert!(source.source() == &token);
    let loaded = repo.load_document(id).unwrap();
    let doc = loaded.artifact();
    assert_eq!(doc.template_id(), tid);
    assert_eq!(doc.template_revision(), source.artifact().revision());
    assert!(doc.created_at_utc() == LATER && doc.updated_at_utc() == LATER);
    let bytes = artifact::encode_document(doc).unwrap();
    assert!(fs::read(document_path(&f, id)).unwrap() == bytes);
    for n in [1, 2] {
        assert!(doc.field_values()[&field(n)].is_unset());
        assert_eq!(
            fixture_owner(&bytes, &["fieldValues", &field(n).to_string()]),
            br#"{"kind":"unset"}"#
        );
    }
    assert!(doc.field_values()[&field(3)].is_unset());
    assert!(
        fixture_owner(&bytes, &["fieldValues", &field(3).to_string()]) == br#"{"kind":"unset"}"#
    );
    let snapshot = doc.orphaned_field_definitions().get(&field(3)).unwrap();
    assert!(snapshot.label() == "fixture field 3");
    let save = artifact::prepare_document_save(
        source.artifact(),
        source.artifact().revision(),
        doc,
        &DocumentEditSet::default(),
        "2026-09-10T00:00:00.000Z",
    )
    .unwrap();
    assert_eq!(save.kind(), DocumentSaveOutcomeKind::Unchanged);
    assert!(artifact::encode_document(save.document()).unwrap() == bytes);
    assert!(save.document().created_at_utc() == LATER && save.document().updated_at_utc() == LATER);
    assert!(
        fixture_owner(
            &artifact::encode_document(save.document()).unwrap(),
            &["orphanedFieldDefinitions"]
        ) == fixture_owner(&bytes, &["orphanedFieldDefinitions"])
    );
    let mat = artifact::materialize_document(
        source.artifact(),
        source.artifact().revision(),
        doc,
        "2026-09-10T00:00:00.000Z".into(),
    )
    .unwrap();
    assert_eq!(mat.kind(), DocumentMaterializationOutcomeKind::Unchanged);
    assert!(mat.document().is_none());
    assert!(save.warnings() == mat.warnings());
    assert!(save.warnings().is_empty());
    assert!(artifact::encode_document(doc).unwrap() == bytes);
    assert!(doc.created_at_utc() == LATER && doc.updated_at_utc() == LATER);
    assert!(fs::read(template_path(&f, tid)).unwrap() == original);
    assert!(
        fs::metadata(template_path(&f, tid))
            .unwrap()
            .modified()
            .unwrap()
            == mtime
    );
    assert!(fs::read(document_path(&f, id)).unwrap() == bytes);
    drop(repo);
    drop(ready);
    rt.close().unwrap();
}

#[test]
fn g7_source_target_and_context_rejections_preserve_owners() {
    for case in [
        "missing",
        "future",
        "invalid",
        "wrong-id",
        "occupied",
        "occupied-future",
        "occupied-corrupt",
        "occupied-wrong-id",
        "wrong-project",
        "wrong-target",
        "wrong-session",
    ] {
        let f = Fixture::new();
        let tid = fixture_seed(&f);
        let mut rt = f.runtime();
        let input = fixture_input(&mut rt, tid);
        let name_ptr = input.name.as_ptr();
        let mut ticket = prepare_create_document(&mut rt, &input).unwrap();
        let id = ticket.document_id();
        let candidate = ticket.candidate() as *const _;
        let candidate_bytes = artifact::encode_document(ticket.candidate()).unwrap();
        let payload = CallerPayload::new(73);
        let payload_ptr = &*payload.0 as *const _;
        let owners = OwnerProof::capture(&input, &ticket, &payload);
        let mut expected_source = fs::read(template_path(&f, tid)).unwrap();
        let mut occupant = None;
        match case {
            "missing" => {
                fs::remove_file(template_path(&f, tid)).unwrap();
            }
            "future" | "invalid" | "wrong-id" => {
                let mut raw: serde_json::Value = serde_json::from_slice(&expected_source).unwrap();
                match case {
                    "future" => raw["schemaVersion"] = 99.into(),
                    "wrong-id" => raw["templateId"] = TemplateId::new().to_string().into(),
                    _ => raw["fields"][field(1).to_string()]["kind"] = "invalid".into(),
                }
                expected_source = serde_json::to_vec(&raw).unwrap();
                fs::write(template_path(&f, tid), &expected_source).unwrap();
            }
            c if c.starts_with("occupied") => {
                let mut raw: serde_json::Value = serde_json::from_slice(&candidate_bytes).unwrap();
                raw["name"] = "independent occupant".into();
                if case == "occupied-future" {
                    raw["schemaVersion"] = 99.into();
                }
                if case == "occupied-wrong-id" {
                    raw["documentId"] = DocumentId::new().to_string().into();
                }
                let bytes = if case == "occupied-corrupt" {
                    b"{".to_vec()
                } else {
                    serde_json::to_vec(&raw).unwrap()
                };
                if case == "occupied" {
                    assert!(artifact::decode_document(&bytes).is_ok());
                }
                fs::create_dir(f.root.join("documents")).unwrap();
                fs::write(document_path(&f, id), &bytes).unwrap();
                occupant = Some(bytes);
            }
            _ => {}
        }
        let mut session = begin::<CallerPayload>(&rt, ticket.session_targets());
        let mut other: Session = begin(&rt, ticket.session_targets());
        let other_snap = other.snapshot();
        if case == "wrong-target" {
            session
                .change_targets(vec![ArtifactSourceId::Document(DocumentId::new())
                    .path()
                    .unwrap()])
                .unwrap();
        }
        let snap = session.snapshot();
        let ctx = TemplateWriteContext {
            project: if case == "wrong-project" {
                "different project"
            } else {
                snap.project_fingerprint().unwrap()
            },
            session: if case == "wrong-session" {
                other_snap.session_id().unwrap()
            } else {
                snap.session_id().unwrap()
            },
        };
        let ((result, c, k), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(|_| panic!("rejection must precede namespace")),
            )),
            false,
            || observe(|| create_document_from_template(&mut rt, &mut session, ctx, &mut ticket)),
        );
        assert_eq!(hooks.hooks, 0);
        assert_no_io(&result, c, k);
        assert_eq!(result.diagnostic().disk, DiskState::NotAttempted);
        let expected = match case {
            "occupied" => ApplicationCategory::DomainRejected,
            "wrong-project" => ApplicationCategory::ProjectMismatch,
            "wrong-target" => ApplicationCategory::SessionTargetsMismatch,
            "wrong-session" => ApplicationCategory::SessionMismatch,
            _ => ApplicationCategory::RepositoryRejected,
        };
        assert_eq!(
            result.diagnostic().category,
            Some(expected),
            "case {case}: {result:?}"
        );
        if case == "occupied" {
            assert!(
                matches!(result.body(),Some(BodyOutcome::Rejected(e)) if matches!(e.domain_cause(),Some(DocumentUseCaseError::TargetOccupied)))
            );
        }
        assert_eq!(ticket.state(), CreationState::Uncommitted);
        assert_eq!(ticket.document_id(), id);
        assert!(std::ptr::eq(ticket.candidate(), candidate));
        assert!(artifact::encode_document(ticket.candidate()).unwrap() == candidate_bytes);
        assert!(input.name.as_ptr() == name_ptr && std::ptr::eq(&*payload.0, payload_ptr));
        if case != "missing" {
            assert!(fs::read(template_path(&f, tid)).unwrap() == expected_source);
        } else {
            assert!(!template_path(&f, tid).exists());
        }
        owners.assert_preserved(&input, &ticket, &payload);
        owners.assert_caller_drop(payload);
        if let Some(bytes) = occupant {
            assert!(fs::read(document_path(&f, id)).unwrap() == bytes);
        } else {
            assert!(!f.root.join("documents").exists());
        }
        assert_eq!(fixture_journals(&f), 0);
        assert!(!format!("{input:?} {ticket:?} {result:?}").contains("G7_FIX_SYNTHETIC"));
        session.end_edit().unwrap();
        other.end_edit().unwrap();
        rt.close().unwrap();
    }
    for case in [
        "foreign-source",
        "revision",
        "required-unset",
        "initial-deleted",
    ] {
        let f = Fixture::new();
        let tid = fixture_seed(&f);
        let mut rt = f.runtime();
        let mut input = fixture_input(&mut rt, tid);
        let foreign = Fixture::new();
        let foreign_tid = fixture_seed(&foreign);
        let mut foreign_rt = foreign.runtime();
        match case {
            "foreign-source" => input.source = source(&mut foreign_rt, foreign_tid),
            "revision" => {
                input.source.expected_revision = artifact::TemplateRevision::try_from(12).unwrap()
            }
            _ => {
                let mut raw: serde_json::Value =
                    serde_json::from_slice(&fs::read(template_path(&f, tid)).unwrap()).unwrap();
                if case == "required-unset" {
                    raw["fields"][field(1).to_string()]["required"] = serde_json::json!(true);
                    raw["fields"][field(1).to_string()]["defaultValue"] =
                        serde_json::json!({"kind":"unset"});
                } else {
                    raw["lifecycle"] = "deleted".into();
                }
                fs::write(template_path(&f, tid), serde_json::to_vec(&raw).unwrap()).unwrap();
                input.source = source(&mut rt, tid);
            }
        }
        let original = fs::read(template_path(&f, tid)).unwrap();
        let foreign_original = fs::read(template_path(&foreign, foreign_tid)).unwrap();
        let name = input.name.clone();
        let time = input.timestamp_utc.clone();
        let name_ptr = input.name.as_ptr();
        let token = input.source.token.clone();
        let payload = CallerPayload::new(74);
        let payload_ptr = &*payload.0 as *const _;
        let drops = Arc::clone(&payload.1);
        let ((error, counts, commits), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(|_| panic!("source admission must precede namespace")),
            )),
            false,
            || observe(|| prepare_create_document(&mut rt, &input).unwrap_err()),
        );
        assert_eq!(
            (counts.calls, counts.allocations, commits, hooks.hooks),
            (0, 0, 0, 0)
        );
        assert!(
            input.name == name && input.timestamp_utc == time && input.name.as_ptr() == name_ptr
        );
        assert!(input.source.token == token);
        assert!(std::ptr::eq(&*payload.0, payload_ptr));
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        assert!(fs::read(template_path(&f, tid)).unwrap() == original);
        assert!(fs::read(template_path(&foreign, foreign_tid)).unwrap() == foreign_original);
        drop(payload);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(fixture_journals(&f), 0);
        match case {
            "foreign-source" => assert!(
                matches!(error,DocumentPreparationError::Source(ref e) if matches!(e.domain_cause(),Some(DocumentUseCaseError::SourceMismatch)))
            ),
            "revision" => assert!(
                matches!(error,DocumentPreparationError::Source(ref e) if matches!(e.domain_cause(),Some(DocumentUseCaseError::RevisionMismatch)))
            ),
            "required-unset" => assert!(
                matches!(error,DocumentPreparationError::Domain(DocumentUseCaseError::Creation(e)) if e.category()==DocumentCreationErrorCategory::RequiredValueUnset)
            ),
            _ => assert!(
                matches!(error,DocumentPreparationError::Domain(DocumentUseCaseError::Creation(e)) if e.category()==DocumentCreationErrorCategory::TemplateIsTombstoned)
            ),
        }
        assert!(!f.root.join("documents").exists());
        foreign_rt.close().unwrap();
        rt.close().unwrap();
    }
}

struct ObservedProvider {
    inner: NoLockService,
    calls: AtomicUsize,
    fail: AtomicUsize,
}
impl LockService for ObservedProvider {
    fn provider_info(&self) -> LockProviderInfo {
        self.inner.provider_info()
    }
    fn acquire(&self, r: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        self.inner.acquire(r)
    }
    fn validate(&self, h: &mut dyn HeldLock) -> Result<(), LockError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.fail.load(Ordering::SeqCst) {
            Err(LockError::for_held(
                LockErrorCategory::LockLost,
                h.provider_kind(),
                LockOperation::Validate,
                h.project_fingerprint(),
                h.session_id(),
                h.target(),
            ))
        } else {
            self.inner.validate(h)
        }
    }
    fn release(&self, h: &mut dyn HeldLock) -> Result<(), LockError> {
        self.inner.release(h)
    }
}
#[test]
fn g7_actual_permit_loss_before_namespace() {
    // 정상 호출에서 namespace hook이 두 번째 provider 검증 뒤에 놓이는지 먼저 확인한다.
    // 실패 control도 실제 source decode 1회를 통과해야 하며 호출 횟수만 맞춰서는 통과하지 못한다.
    for fail_before_namespace in [false, true] {
        let f = Fixture::new();
        let tid = fixture_seed(&f);
        let mut rt = f.runtime();
        let input = fixture_input(&mut rt, tid);
        let original = fs::read(template_path(&f, tid)).unwrap();
        let mut ticket = prepare_create_document(&mut rt, &input).unwrap();
        let payload = CallerPayload::new(75);
        let owners = OwnerProof::capture(&input, &ticket, &payload);
        let provider = Arc::new(ObservedProvider {
            inner: NoLockService::new(),
            calls: AtomicUsize::new(0),
            fail: AtomicUsize::new(0),
        });
        let mut session = EditSessionService::<CallerPayload, ()>::new(provider.clone());
        session
            .begin_edit(rt.project_fingerprint(), ticket.session_targets().to_vec())
            .unwrap();
        provider.calls.store(0, Ordering::SeqCst);
        provider
            .fail
            .store(if fail_before_namespace { 2 } else { 0 }, Ordering::SeqCst);
        let at_namespace = Arc::clone(&provider);
        let snap = session.snapshot();
        let ((result, c, k), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(move |target| {
                    assert!(
                        !fail_before_namespace,
                        "namespace must not be reached after permit loss"
                    );
                    assert!(target.file_name().unwrap() == "documents");
                    assert_eq!(at_namespace.calls.load(Ordering::SeqCst), 2);
                    Ok(())
                }),
            )),
            false,
            || {
                observe(|| {
                    create_document_from_template(
                        &mut rt,
                        &mut session,
                        context(&snap),
                        &mut ticket,
                    )
                })
            },
        );
        assert_eq!(hooks.decoded, 1);
        if fail_before_namespace {
            assert_no_io(&result, c, k);
            assert_eq!(hooks.hooks, 0);
            assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
            assert_eq!(session.snapshot().state(), EditSessionState::LockLost);
            assert_eq!(
                result.diagnostic().permit.unwrap().lock_category,
                Some(LockErrorCategory::LockLost)
            );
            assert!(matches!(
                result.body(),
                Some(BodyOutcome::Write {
                    transaction: TransactionResult::PrepareFailed(_),
                    ..
                })
            ));
            assert_eq!(ticket.state(), CreationState::Uncommitted);
            assert!(!f.root.join("documents").exists());
            assert!(!document_path(&f, ticket.document_id()).exists());
        } else {
            assert_eq!(hooks.hooks, 1);
            assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
            assert_eq!(result.diagnostic().disk, DiskState::Committed);
            assert_eq!(ticket.state(), CreationState::Committed);
            assert!(fs::read(document_path(&f, ticket.document_id())).unwrap() == owners.bytes);
        }
        assert_eq!(fixture_journals(&f), 0);
        assert!(fs::read(template_path(&f, tid)).unwrap() == original);
        owners.assert_preserved(&input, &ticket, &payload);
        owners.assert_caller_drop(payload);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g7_namespace_races_use_document_admission() {
    for kind in ["directory", "file", "junction", "denied"] {
        let f = Fixture::new();
        let tid = fixture_seed(&f);
        let mut rt = f.runtime();
        let input = fixture_input(&mut rt, tid);
        let original = fs::read(template_path(&f, tid)).unwrap();
        let mut ticket = prepare_create_document(&mut rt, &input).unwrap();
        let payload = CallerPayload::new(77);
        let owners = OwnerProof::capture(&input, &ticket, &payload);
        let mut session = begin::<CallerPayload>(&rt, ticket.session_targets());
        let snap = session.snapshot();
        let outside = f.base.join("outside");
        fs::create_dir(&outside).unwrap();
        let outside_marker = outside.join("preserved.txt");
        fs::write(&outside_marker, b"outside namespace marker").unwrap();
        let ((result, c, k), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(move |target| {
                    assert!(target.file_name().unwrap() == "documents");
                    match kind {
                        "directory" => fs::create_dir(target)?,
                        "file" => fs::write(target, b"fixture occupied namespace")?,
                        "junction" => {
                            let out = std::process::Command::new("cmd.exe")
                                .args(["/C", "mklink", "/J"])
                                .arg(target)
                                .arg(&outside)
                                .output()?;
                            assert!(out.status.success());
                        }
                        _ => return Err(io::Error::from_raw_os_error(5)),
                    }
                    Ok(())
                }),
            )),
            false,
            || {
                observe(|| {
                    create_document_from_template(
                        &mut rt,
                        &mut session,
                        context(&snap),
                        &mut ticket,
                    )
                })
            },
        );
        assert_eq!(hooks.hooks, 1);
        if kind == "directory" {
            assert_eq!(result.diagnostic().disk, DiskState::Committed);
            assert_eq!((c.calls, c.allocations, k), (1, 1, 1));
        } else {
            assert_no_io(&result, c, k);
            assert_eq!(result.diagnostic().disk, DiskState::NotApplied);
            assert!(!document_path(&f, ticket.document_id()).exists());
            assert_eq!(fixture_journals(&f), 0);
            assert_eq!(ticket.state(), CreationState::Uncommitted);
            if kind == "file" {
                assert!(
                    fs::read(f.root.join("documents")).unwrap() == b"fixture occupied namespace"
                );
            }
        }
        assert!(fs::read(&outside_marker).unwrap() == b"outside namespace marker");
        assert!(fs::read(template_path(&f, tid)).unwrap() == original);
        owners.assert_preserved(&input, &ticket, &payload);
        owners.assert_caller_drop(payload);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g7_private_byte_assertions_retain_failure_without_operand_dump() {
    let left = b"G7_PRIVATE_ASSERTION_MARKER".to_vec();
    let right = b"different synthetic value".to_vec();
    assert!(left == left.clone());
    assert!(left != right);
    // panic hook을 교체하지 않으므로 다른 thread의 진단이나 RAII fault 상태에 간섭하지 않는다.
    let failures = [
        std::panic::catch_unwind(|| assert!(left == right)).unwrap_err(),
        std::panic::catch_unwind(|| assert!(left != left.clone())).unwrap_err(),
    ];
    for failure in failures {
        let message = failure
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| failure.downcast_ref::<&str>().copied())
            .unwrap();
        assert!(message.contains("assertion failed"));
        assert!(!message.contains("G7_PRIVATE_ASSERTION_MARKER"));
        assert!(!message.contains(&format!("{left:?}")));
        assert!(!message.contains(&format!("{right:?}")));
    }
}

#[test]
fn g7_clean_prepare_then_real_committed_cleanup_and_recovery() {
    let f = Fixture::new();
    let tid = fixture_seed(&f);
    let mut rt = f.runtime();
    let input = fixture_input(&mut rt, tid);
    let original = fs::read(template_path(&f, tid)).unwrap();
    let name_ptr = input.name.as_ptr();
    let mut ticket = prepare_create_document(&mut rt, &input).unwrap();
    let id = ticket.document_id();
    let candidate = ticket.candidate() as *const _;
    let expected = artifact::encode_document(ticket.candidate()).unwrap();
    let payload = CallerPayload::new(91);
    let payload_ptr = &*payload.0 as *const _;
    let owners = OwnerProof::capture(&input, &ticket, &payload);
    let mut session = begin::<CallerPayload>(&rt, ticket.session_targets());
    let snap = session.snapshot();
    let commits = Rc::new(Cell::new(0));
    let kc = commits.clone();
    let cause_owner = Arc::new(());
    let inject_owner = Arc::clone(&cause_owner);
    let (failed, c) = with_canonical_prepare_hooks(
        move |p, _| {
            if p == PrepareFailPoint::StagedWrite {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    OwnedCause(Arc::clone(&inject_owner)),
                ))
            } else {
                Ok(())
            }
        },
        || {
            with_commit_io_factory(
                move |p, _| {
                    if p == Some(CommitTestPoint::ManifestRevalidation) {
                        kc.set(kc.get() + 1);
                    }
                    Ok(())
                },
                || {
                    create_document_from_template(
                        &mut rt,
                        &mut session,
                        context(&snap),
                        &mut ticket,
                    )
                },
            )
        },
    );
    assert_eq!((c.calls, c.allocations, commits.get()), (1, 1, 0));
    assert_eq!(failed.diagnostic().disk, DiskState::NotApplied);
    assert_eq!(
        failed.diagnostic().artifact_write.unwrap().io.unwrap().kind,
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(ticket.state(), CreationState::Uncommitted);
    assert!(std::ptr::eq(ticket.candidate(), candidate));
    assert!(!document_path(&f, id).exists());
    assert_eq!(fixture_journals(&f), 0);
    assert_eq!(fixture_count_docs(&f), 0);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert!(input.name.as_ptr() == name_ptr && std::ptr::eq(&*payload.0, payload_ptr));
    assert!(fs::read(template_path(&f, tid)).unwrap() == original);
    assert!(!format!("{failed:?}").contains("G7_FIX_CAUSE"));
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::PrepareFailed(error),
        ..
    }) = failed.body()
    else {
        panic!("expected the actual retained prepare error");
    };
    let io = error
        .implementation_original_prepare()
        .unwrap()
        .implementation_original_io()
        .unwrap();
    let cause = crate::data::transaction::io_cause(io)
        .get_ref()
        .unwrap()
        .downcast_ref::<OwnedCause>()
        .unwrap();
    assert!(Arc::ptr_eq(&cause.0, &cause_owner));
    owners.assert_preserved(&input, &ticket, &payload);
    assert_eq!(Arc::strong_count(&cause_owner), 2);
    drop(failed);
    assert_eq!(Arc::strong_count(&cause_owner), 1);
    let held = Rc::new(RefCell::new(None));
    let h = held.clone();
    let root = f.root.join(".worldbuild/transactions");
    let reached = Rc::new(Cell::new(0));
    let rc = reached.clone();
    // handle은 이 호출 밖의 RAII owner에 두어 cleanup 실패를 유지하고 recovery 전에 해제한다.
    let commit_entries = Rc::new(Cell::new(0));
    let commit_counter = Rc::clone(&commit_entries);
    let (result, c) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        commit_counter.set(commit_counter.get() + 1);
                    }
                    if point == Some(CommitTestPoint::Cleanup) {
                        let p = fs::read_dir(&root)?
                            .next()
                            .ok_or_else(|| io::Error::other("expected journal"))??;
                        assert!(p.path().join("committed.json").exists());
                        *h.borrow_mut() = Some(
                            fs::OpenOptions::new()
                                .read(true)
                                .share_mode(1)
                                .open(p.path().join("manifest.json"))?,
                        );
                        rc.set(rc.get() + 1);
                    }
                    Ok(())
                },
                || {
                    create_document_from_template(
                        &mut rt,
                        &mut session,
                        context(&snap),
                        &mut ticket,
                    )
                },
            )
        },
    );
    assert_eq!(
        (c.calls, c.allocations, commit_entries.get(), reached.get()),
        (1, 1, 1, 1)
    );
    assert_eq!(result.diagnostic().disk, DiskState::Committed);
    assert_eq!(
        result.diagnostic().artifact_commit.unwrap().state,
        CommitResultState::CommittedCleanupFailed
    );
    assert_eq!(
        result
            .diagnostic()
            .artifact_commit
            .unwrap()
            .io
            .unwrap()
            .os_code,
        Some(32)
    );
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = result.body()
    else {
        panic!("expected committed cleanup failure with original cause");
    };
    let crate::data::transaction::CommitOutcome::CommittedCleanupFailed {
        failure,
        cleanup_failure,
    } = outcome.implementation_original()
    else {
        panic!("expected original committed cleanup failure");
    };
    let crate::data::transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
        panic!("expected actual cleanup I/O cause");
    };
    // cleanup은 OwnedFailure도 감싼다. 기존 진단의 닫힌 wrapper 순회를 사용하고 원 객체는 결과에서 빌린다.
    let cleanup_owner = io as *const io::Error;
    assert_eq!(
        crate::data::transaction::artifact_diagnostics::IoDiagnostic::new(io).os_code,
        Some(32)
    );
    assert!(cleanup_failure.is_none());
    assert!(result.diagnostic().recovery_required);
    owners.assert_preserved(&input, &ticket, &payload);
    assert_eq!(ticket.state(), CreationState::Committed);
    assert_eq!(rt.snapshot().state, RuntimeState::Pending);
    assert_eq!(fixture_journals(&f), 1);
    assert!(fs::read(document_path(&f, id)).unwrap() == expected);
    assert_eq!(fixture_count_docs(&f), 1);
    let (again, c, k) = observe(|| {
        create_document_from_template(&mut rt, &mut session, context(&snap), &mut ticket)
    });
    assert_no_io(&again, c, k);
    owners.assert_preserved(&input, &ticket, &payload);
    assert_eq!(
        again.diagnostic().category,
        Some(ApplicationCategory::RuntimeRejected)
    );
    assert_eq!(ticket.state(), CreationState::Committed);
    let (wrong, c, k) = observe(|| {
        create_document_from_template(
            &mut rt,
            &mut session,
            TemplateWriteContext {
                project: "wrong",
                session: snap.session_id().unwrap(),
            },
            &mut ticket,
        )
    });
    assert_no_io(&wrong, c, k);
    assert_eq!(
        wrong.diagnostic().category,
        Some(ApplicationCategory::ProjectMismatch)
    );
    owners.assert_preserved(&input, &ticket, &payload);
    assert_eq!(ticket.state(), CreationState::Committed);
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(retained)),
        ..
    }) = result.body()
    else {
        panic!("cleanup result owner must remain available");
    };
    let retained_failure = retained.implementation_original().failures().unwrap().0;
    let crate::data::transaction::CommitFailureSource::Io(retained_io) =
        retained_failure.source.as_ref()
    else {
        panic!("cleanup cause owner must remain available");
    };
    assert!(std::ptr::eq(retained_io, cleanup_owner));
    held.borrow_mut().take();
    rt.recover().unwrap();
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(fixture_journals(&f), 0);
    {
        let ready = rt.ready().unwrap();
        let repository = ArtifactRepository::new(&ready).unwrap();
        let document = repository.load_document(id).unwrap();
        assert!(artifact::encode_document(document.artifact()).unwrap() == expected);
        assert!(document.artifact().name() == input.name);
        assert_eq!(repository.scan_documents().unwrap().sources().count(), 1);
    }
    let (again, c, k) = observe(|| {
        create_document_from_template(&mut rt, &mut session, context(&snap), &mut ticket)
    });
    assert_no_io(&again, c, k);
    assert!(
        matches!(again.body(),Some(BodyOutcome::Rejected(e)) if matches!(e.domain_cause(),Some(DocumentUseCaseError::PreparationAlreadyCommitted)))
    );
    assert_eq!(ticket.state(), CreationState::Committed);
    assert_eq!(ticket.document_id(), id);
    assert_eq!(fixture_count_docs(&f), 1);
    assert!(fs::read(document_path(&f, id)).unwrap() == expected);
    assert!(fs::read(template_path(&f, tid)).unwrap() == original);
    assert!(
        input.name.as_ptr() == name_ptr
            && std::ptr::eq(&*payload.0, payload_ptr)
            && std::ptr::eq(ticket.candidate(), candidate)
    );
    owners.assert_preserved(&input, &ticket, &payload);
    owners.assert_caller_drop(payload);
    session.end_edit().unwrap();
    rt.close().unwrap();
}
