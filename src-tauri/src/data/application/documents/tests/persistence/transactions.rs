use super::*;
use crate::data::{
    application::write::TransactionResult,
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
        LockProviderInfo, LockService,
    },
    storage_estimate::test_support::{with_storage_response, TestStorageResponse},
    transaction::{CommitResultState, PrepareFailPoint},
};
use std::{cell::RefCell, io, os::windows::fs::OpenOptionsExt};

#[derive(Debug)]
struct OwnedCause(Arc<()>);
impl std::fmt::Display for OwnedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("G8_PRIVATE_IO_CAUSE")
    }
}
impl std::error::Error for OwnedCause {}

fn journals(f: &Fixture) -> usize {
    let path = f.root.join(".worldbuild/transactions");
    if !path.exists() {
        return 0;
    }
    fs::read_dir(path)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .len()
}
fn expected(rt: &mut ProjectRuntime, input: &SaveDocumentInput) -> Vec<u8> {
    let ready = rt.ready().unwrap();
    let repo = ArtifactRepository::new(&ready).unwrap();
    let doc = repo.load_document(input.document.id).unwrap();
    let template = repo.load_template(input.template.id).unwrap();
    let outcome = artifact::prepare_document_save(
        template.artifact(),
        input.template.expected_revision,
        doc.artifact(),
        &input.edits,
        &input.timestamp_utc,
    )
    .unwrap();
    assert_warning(outcome.warnings());
    artifact::encode_document(outcome.document()).unwrap()
}
fn candidate(result: &DocumentUpdateExecution<DocumentSaveOutcome>, bytes: &[u8]) {
    let owner = result.outcome().expect("owned pure outcome retained");
    assert_eq!(owner.kind(), DocumentSaveOutcomeKind::Changed);
    assert_warning(owner.warnings());
    assert!(
        artifact::encode_document(owner.document()).unwrap() == bytes,
        "retained candidate bytes"
    );
    assert!(owner.document().updated_at_utc() == SAVE_TIME);
    assert_numbers(bytes);
    assert!(!format!("{result:?}").contains(PRIVATE));
    assert!(!format!("{result:?}").contains("G8_PRIVATE_IO_CAUSE"));
}

#[test]
fn g8_context_and_ready_rejections_cover_empty_save_without_namespace_or_candidate() {
    for case in [
        "project",
        "session",
        "target",
        "multiple-targets",
        "not-ready",
    ] {
        let f = Fixture::new();
        let (tid, did) = seed(&f);
        let mut rt = f.runtime();
        let input = save_input(&mut rt, tid, did, vec![]);
        let payload = Payload::new();
        let proof = RequestProof::capture(&input, &payload);
        if case == "not-ready" {
            rt.close().unwrap();
            rt = ProjectRuntime::acquire(&f.root, &f.base.join("locks")).unwrap();
            assert_eq!(rt.snapshot().state, RuntimeState::Pending);
        }
        let db = disk(&document_path(&f, did));
        let tb = disk(&template_path(&f, tid));
        let targets = doc_targets(did);
        let mut session = begin::<Payload>(&rt, targets.session_targets());
        if case == "target" {
            session
                .change_targets(vec![ArtifactSourceId::Template(tid).path().unwrap()])
                .unwrap();
        }
        if case == "multiple-targets" {
            session
                .change_targets(vec![
                    ArtifactSourceId::Template(tid).path().unwrap(),
                    ArtifactSourceId::Document(did).path().unwrap(),
                ])
                .unwrap();
        }
        let mut other: Session = begin(&rt, targets.session_targets());
        let other_snap = other.snapshot();
        let snap = session.snapshot();
        let ctx = TemplateWriteContext {
            project: if case == "project" {
                "other"
            } else {
                snap.project_fingerprint().unwrap()
            },
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
                Box::new(|_| panic!("no namespace before authority")),
            )),
            false,
            || observe(|| save_document(&mut rt, &mut session, ctx, &input).unwrap()),
        );
        no_io(c, k);
        assert_eq!((hooks.decoded, hooks.hooks), (0, 0));
        assert!(result.outcome().is_none());
        assert_eq!(
            result.execution.diagnostic().category,
            Some(match case {
                "project" => ApplicationCategory::ProjectMismatch,
                "session" => ApplicationCategory::SessionMismatch,
                "not-ready" => ApplicationCategory::RuntimeRejected,
                _ => ApplicationCategory::SessionTargetsMismatch,
            })
        );
        assert_eq!(result.execution.diagnostic().disk, DiskState::NotAttempted);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        assert_eq!(
            rt.snapshot().state,
            if case == "not-ready" {
                RuntimeState::Pending
            } else {
                RuntimeState::Ready
            }
        );
        assert_disk(&document_path(&f, did), &db);
        assert_disk(&template_path(&f, tid), &tb);
        assert_eq!(journals(&f), 0);
        proof.assert(&input, &payload);
        proof.drop_payload(payload);
        session.end_edit().unwrap();
        other.end_edit().unwrap();
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
fn g8_actual_replace_provider_order_and_pre_body_or_prepare_permit_loss_preserve_owners() {
    let mut observed_before_original = None;
    // 성공 Replace를 먼저 관측한 후 해당 실제 prepare 검증 지점에서 실패시킨다.
    for case in ["normal", "before-body", "prepare", "empty-before-body"] {
        let f = Fixture::new();
        let (tid, did) = seed(&f);
        let mut rt = f.runtime();
        let input = save_input(
            &mut rt,
            tid,
            did,
            if case == "empty-before-body" {
                vec![]
            } else {
                vec![DocumentEdit::Rename("changed".into())]
            },
        );
        let expected = if case == "empty-before-body" {
            None
        } else {
            Some(expected(&mut rt, &input))
        };
        let payload = Payload::new();
        let proof = RequestProof::capture(&input, &payload);
        let db = disk(&document_path(&f, did));
        let tb = disk(&template_path(&f, tid));
        let targets = doc_targets(did);
        let provider = Arc::new(ObservedProvider {
            inner: NoLockService::new(),
            calls: AtomicUsize::new(0),
            fail: AtomicUsize::new(0),
        });
        let mut session = EditSessionService::<Payload, ()>::new(provider.clone());
        session
            .begin_edit(rt.project_fingerprint(), targets.session_targets().to_vec())
            .unwrap();
        provider.calls.store(0, Ordering::SeqCst);
        provider.fail.store(
            match case {
                "before-body" | "empty-before-body" => 1,
                "prepare" => observed_before_original.unwrap(),
                _ => 0,
            },
            Ordering::SeqCst,
        );
        let at_original = Rc::new(Cell::new(0));
        let original_count = at_original.clone();
        let p = provider.clone();
        let commits = Rc::new(Cell::new(0));
        let kc = commits.clone();
        let at_commit = Rc::new(Cell::new(0));
        let ac = at_commit.clone();
        let pc = provider.clone();
        let snap = session.snapshot();
        let ((result, c), hooks) = repo_hooks::scoped(
            Some((
                RepositoryStage::Namespace,
                0,
                Box::new(|_| panic!("Replace must not create namespace")),
            )),
            false,
            || {
                with_canonical_prepare_hooks(
                    move |point, _| {
                        if point == PrepareFailPoint::BeforeOriginalRead {
                            original_count.set(p.calls.load(Ordering::SeqCst));
                        }
                        Ok(())
                    },
                    || {
                        with_commit_io_factory(
                            move |point, _| {
                                if point == Some(CommitTestPoint::ManifestRevalidation) {
                                    kc.set(kc.get() + 1);
                                    ac.set(pc.calls.load(Ordering::SeqCst));
                                }
                                Ok(())
                            },
                            || {
                                save_document(&mut rt, &mut session, context(&snap), &input)
                                    .unwrap()
                            },
                        )
                    },
                )
            },
        );
        assert_eq!(hooks.hooks, 0);
        if case == "normal" {
            assert_eq!((c.calls, c.allocations, commits.get()), (1, 1, 1));
            assert_eq!(hooks.decoded, 2);
            assert_eq!(result.execution.diagnostic().disk, DiskState::Committed);
            assert_eq!(
                (
                    at_original.get(),
                    at_commit.get(),
                    provider.calls.load(Ordering::SeqCst)
                ),
                (2, 3, 5)
            );
            observed_before_original = Some(at_original.get());
            println!(
                "g8-replace-provider original={} commit={} total={}",
                at_original.get(),
                at_commit.get(),
                provider.calls.load(Ordering::SeqCst)
            );
            candidate(&result, expected.as_ref().unwrap());
            assert!(fs::read(document_path(&f, did)).unwrap() == *expected.as_ref().unwrap());
        } else {
            assert_eq!(
                result.execution.diagnostic().permit.unwrap().lock_category,
                Some(LockErrorCategory::LockLost)
            );
            assert_eq!(session.snapshot().state(), EditSessionState::LockLost);
            if case == "prepare" {
                assert_eq!((c.calls, c.allocations, commits.get()), (1, 0, 0));
                assert_eq!(hooks.decoded, 2);
                candidate(&result, expected.as_ref().unwrap());
                assert!(matches!(
                    result.execution.body(),
                    Some(BodyOutcome::Write {
                        transaction: TransactionResult::PrepareFailed(_),
                        ..
                    })
                ));
            } else {
                no_io(c, commits.get());
                assert_eq!(hooks.decoded, 0);
                assert!(result.outcome().is_none());
            }
            assert_disk(&document_path(&f, did), &db);
            let (again, c, k) =
                observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
            no_io(c, k);
            assert_eq!(
                again.execution.diagnostic().category,
                Some(ApplicationCategory::InvalidSessionState)
            );
            assert!(again.outcome().is_none());
        }
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(journals(&f), 0);
        assert_disk(&template_path(&f, tid), &tb);
        proof.assert(&input, &payload);
        proof.drop_payload(payload);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g8_space_and_clean_prepare_failures_retain_candidate_warning_input_and_original_io_owner() {
    for space_denied in [true, false] {
        let f = Fixture::new();
        let (tid, did) = seed(&f);
        let mut rt = f.runtime();
        let input = save_input(
            &mut rt,
            tid,
            did,
            vec![DocumentEdit::Rename("changed".into())],
        );
        let expected = expected(&mut rt, &input);
        let payload = Payload::new();
        let proof = RequestProof::capture(&input, &payload);
        let db = disk(&document_path(&f, did));
        let tb = disk(&template_path(&f, tid));
        let targets = doc_targets(did);
        let mut session = begin::<Payload>(&rt, targets.session_targets());
        let snap = session.snapshot();
        let cause_owner = Arc::new(());
        let injected = cause_owner.clone();
        let commits = Rc::new(Cell::new(0));
        let kc = commits.clone();
        let execute = || {
            with_canonical_prepare_hooks(
                move |point, _| {
                    if !space_denied && point == PrepareFailPoint::StagedWrite {
                        Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            OwnedCause(injected.clone()),
                        ))
                    } else {
                        Ok(())
                    }
                },
                || {
                    with_commit_io_factory(
                        move |point, _| {
                            if point == Some(CommitTestPoint::ManifestRevalidation) {
                                kc.set(kc.get() + 1);
                            }
                            Ok(())
                        },
                        || save_document(&mut rt, &mut session, context(&snap), &input).unwrap(),
                    )
                },
            )
        };
        let (result, c) = if space_denied {
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
        assert_eq!(
            (c.calls, c.allocations, commits.get()),
            (1, usize::from(!space_denied), 0)
        );
        assert_eq!(result.execution.diagnostic().disk, DiskState::NotApplied);
        candidate(&result, &expected);
        let outcome_ptr = result.outcome().unwrap() as *const _;
        let warning_ptr = result.outcome().unwrap().warnings().as_slice().as_ptr();
        let Some(BodyOutcome::Write {
            transaction: TransactionResult::PrepareFailed(error),
            ..
        }) = result.execution.body()
        else {
            panic!("actual prepare failure")
        };
        if !space_denied {
            let original = error
                .implementation_original_prepare()
                .unwrap()
                .implementation_original_io()
                .unwrap();
            let cause = crate::data::transaction::io_cause(original)
                .get_ref()
                .unwrap()
                .downcast_ref::<OwnedCause>()
                .unwrap();
            assert!(Arc::ptr_eq(&cause.0, &cause_owner));
            assert_eq!(Arc::strong_count(&cause_owner), 2);
            assert_eq!(
                result
                    .execution
                    .diagnostic()
                    .artifact_write
                    .unwrap()
                    .io
                    .unwrap()
                    .kind,
                io::ErrorKind::PermissionDenied
            );
        } else {
            assert!(result.execution.diagnostic().artifact_write.is_some());
        }
        assert_disk(&document_path(&f, did), &db);
        assert_disk(&template_path(&f, tid), &tb);
        assert_eq!(journals(&f), 0);
        assert!(
            std::ptr::eq(result.outcome().unwrap(), outcome_ptr)
                && result.outcome().unwrap().warnings().as_slice().as_ptr() == warning_ptr
        );
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        proof.assert(&input, &payload);
        proof.drop_payload(payload);
        drop(result);
        assert_eq!(Arc::strong_count(&cause_owner), 1);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g8_materialize_admission_failure_preserves_changed_candidate_and_nonempty_warning() {
    let f = Fixture::new();
    let (tid, did) = seed(&f);
    let mut rt = f.runtime();
    historical_field(&mut rt, tid, false);
    let input = mat_input(&mut rt, tid, did);
    let token = input.document.token.clone();
    let template_token = input.template.token.clone();
    let time_ptr = input.timestamp_utc.as_ptr();
    let payload = Payload::new();
    let payload_ptr = payload.0.as_ptr();
    let drops = payload.1.clone();
    let db = disk(&document_path(&f, did));
    let tb = disk(&template_path(&f, tid));
    let targets = doc_targets(did);
    let mut session = begin::<Payload>(&rt, targets.session_targets());
    let snap = session.snapshot();
    let ((result, c, k), _) = with_storage_response(
        TestStorageResponse::Available {
            available_bytes: 0,
            allocation_unit_bytes: 4096,
        },
        || observe(|| materialize_document(&mut rt, &mut session, context(&snap), &input).unwrap()),
    );
    assert_eq!((c.calls, c.allocations, k), (1, 0, 0));
    assert_eq!(result.execution.diagnostic().disk, DiskState::NotApplied);
    let outcome = result.outcome().unwrap();
    assert_eq!(outcome.kind(), DocumentMaterializationOutcomeKind::Changed);
    assert_warning(outcome.warnings());
    let bytes = artifact::encode_document(outcome.document().unwrap()).unwrap();
    assert_numbers(&bytes);
    assert_owner(
        &bytes,
        &["fieldValues", &key(3)],
        &json!({"kind":"text","value":"initial"}),
    );
    assert!(outcome.document().unwrap().updated_at_utc() == SAVE_TIME);
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::PrepareFailed(error),
        ..
    }) = result.execution.body()
    else {
        panic!("materialize storage admission cause")
    };
    assert_eq!(
        error.diagnostic().category,
        crate::data::repository::ArtifactWriteCategory::StorageRejected
    );
    assert!(error.implementation_original_prepare().is_some());
    assert!(
        input.document.token == token
            && input.template.token == template_token
            && input.timestamp_utc.as_ptr() == time_ptr
            && input.timestamp_utc == SAVE_TIME
    );
    assert!(payload.0.as_ptr() == payload_ptr && payload.0.as_ref() == PRIVATE.as_bytes());
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    assert_disk(&document_path(&f, did), &db);
    assert_disk(&template_path(&f, tid), &tb);
    assert_eq!(journals(&f), 0);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    drop(payload);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g8_document_changed_after_candidate_is_rejected_by_original_read_without_overwrite() {
    let f = Fixture::new();
    let (tid, did) = seed(&f);
    let mut rt = f.runtime();
    let input = save_input(
        &mut rt,
        tid,
        did,
        vec![DocumentEdit::Rename("changed".into())],
    );
    let expected = expected(&mut rt, &input);
    let payload = Payload::new();
    let proof = RequestProof::capture(&input, &payload);
    let mut raced = fs::read(document_path(&f, did)).unwrap();
    raced.extend_from_slice(b"\n  ");
    let replacement = raced.clone();
    let path = document_path(&f, did);
    let tb = disk(&template_path(&f, tid));
    let race_count = Rc::new(Cell::new(0));
    let rc = race_count.clone();
    let raced_mtime = Rc::new(RefCell::new(None));
    let rm = raced_mtime.clone();
    let targets = doc_targets(did);
    let mut session = begin::<Payload>(&rt, targets.session_targets());
    let snap = session.snapshot();
    let commits = Rc::new(Cell::new(0));
    let kc = commits.clone();
    let (result, c) = with_canonical_prepare_hooks(
        move |point, operation| {
            if point == PrepareFailPoint::BeforeOriginalRead && operation == Some(0) {
                rc.set(rc.get() + 1);
                fs::write(&path, &replacement)?;
                *rm.borrow_mut() = Some(fs::metadata(&path)?.modified()?);
            }
            Ok(())
        },
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        kc.set(kc.get() + 1);
                    }
                    Ok(())
                },
                || save_document(&mut rt, &mut session, context(&snap), &input).unwrap(),
            )
        },
    );
    assert_eq!(race_count.get(), 1);
    assert_eq!((c.calls, c.allocations, commits.get()), (1, 1, 0));
    assert_eq!(result.execution.diagnostic().disk, DiskState::NotApplied);
    candidate(&result, &expected);
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::PrepareFailed(error),
        ..
    }) = result.execution.body()
    else {
        panic!("original read rejects race")
    };
    assert!(error.implementation_original_prepare().is_some());
    assert_eq!(
        error.diagnostic().category,
        crate::data::repository::ArtifactWriteCategory::SourceMismatch
    );
    assert_eq!(
        error.diagnostic().stage,
        crate::data::repository::ArtifactWriteStage::ReadSource
    );
    assert_eq!(
        error.diagnostic().prepare_stage,
        Some(crate::data::transaction::PrepareStage::ReadOriginal)
    );
    assert!(fs::read(document_path(&f, did)).unwrap() == raced);
    assert_eq!(
        fs::metadata(document_path(&f, did))
            .unwrap()
            .modified()
            .unwrap(),
        raced_mtime.borrow().unwrap()
    );
    assert_disk(&template_path(&f, tid), &tb);
    assert_eq!(journals(&f), 0);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    proof.assert(&input, &payload);
    proof.drop_payload(payload);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g8_real_replace_apply_failure_rolls_back_old_bytes_and_keeps_owned_cause() {
    let f = Fixture::new();
    let (tid, did) = seed(&f);
    let mut rt = f.runtime();
    let input = save_input(
        &mut rt,
        tid,
        did,
        vec![DocumentEdit::Rename("changed".into())],
    );
    let expected = expected(&mut rt, &input);
    let payload = Payload::new();
    let proof = RequestProof::capture(&input, &payload);
    let db = disk(&document_path(&f, did));
    let tb = disk(&template_path(&f, tid));
    let targets = doc_targets(did);
    let mut session = begin::<Payload>(&rt, targets.session_targets());
    let snap = session.snapshot();
    let owner = Arc::new(());
    let injected = owner.clone();
    let commits = Rc::new(Cell::new(0));
    let kc = commits.clone();
    let applied = Rc::new(Cell::new(0));
    let a = applied.clone();
    let path = document_path(&f, did);
    let committed_candidate = expected.clone();
    let (result, c) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        kc.set(kc.get() + 1);
                    }
                    // ProgressState에서는 소유 handle이 닫혀 실제 적용 bytes를 읽고 그 뒤 실패시킬 수 있다.
                    if point == Some(CommitTestPoint::ProgressState) {
                        assert!(
                            fs::read(&path)? == committed_candidate,
                            "candidate was really applied before rollback"
                        );
                        a.set(a.get() + 1);
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            OwnedCause(injected.clone()),
                        ));
                    }
                    Ok(())
                },
                || save_document(&mut rt, &mut session, context(&snap), &input).unwrap(),
            )
        },
    );
    assert_eq!(
        (c.calls, c.allocations, commits.get(), applied.get()),
        (1, 1, 1, 1)
    );
    assert_eq!(result.execution.diagnostic().disk, DiskState::RolledBack);
    assert_eq!(
        result.execution.diagnostic().artifact_commit.unwrap().state,
        CommitResultState::RolledBack
    );
    candidate(&result, &expected);
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = result.execution.body()
    else {
        panic!("retained confirmed rollback outcome")
    };
    let (failure, cleanup) = outcome
        .implementation_original()
        .rollback_failures()
        .unwrap();
    assert!(cleanup.is_none());
    let crate::data::transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
        panic!("actual apply I/O cause")
    };
    let cause = crate::data::transaction::io_cause(io)
        .get_ref()
        .unwrap()
        .downcast_ref::<OwnedCause>()
        .unwrap();
    assert!(Arc::ptr_eq(&cause.0, &owner));
    assert!(
        fs::read(document_path(&f, did)).unwrap() == db.0,
        "rollback restores exact old bytes including artifact timestamp"
    );
    let restored = artifact::decode_document(&fs::read(document_path(&f, did)).unwrap()).unwrap();
    assert!(restored.updated_at_utc() == TIME);
    assert_disk(&template_path(&f, tid), &tb);
    assert_eq!(journals(&f), 0);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    proof.assert(&input, &payload);
    proof.drop_payload(payload);
    drop(result);
    assert_eq!(Arc::strong_count(&owner), 1);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g8_real_replace_cleanup_os32_retains_committed_owner_and_requires_recovery_then_fresh_sources() {
    let f = Fixture::new();
    let (tid, did) = seed(&f);
    let mut rt = f.runtime();
    let input = save_input(
        &mut rt,
        tid,
        did,
        vec![DocumentEdit::Rename("changed".into())],
    );
    let expected = expected(&mut rt, &input);
    let payload = Payload::new();
    let proof = RequestProof::capture(&input, &payload);
    let tb = disk(&template_path(&f, tid));
    let targets = doc_targets(did);
    let mut session = begin::<Payload>(&rt, targets.session_targets());
    let snap = session.snapshot();
    let held = Rc::new(RefCell::new(None));
    let handle = held.clone();
    let root = f.root.join(".worldbuild/transactions");
    let cleanup_count = Rc::new(Cell::new(0));
    let cc = cleanup_count.clone();
    let commits = Rc::new(Cell::new(0));
    let kc = commits.clone();
    let (result, c) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        kc.set(kc.get() + 1);
                    }
                    if point == Some(CommitTestPoint::Cleanup) {
                        let journal = fs::read_dir(&root)?
                            .next()
                            .ok_or_else(|| io::Error::other("expected journal"))??;
                        assert!(
                            journal.path().join("committed.json").exists(),
                            "actual committed marker precedes OS sharing failure"
                        );
                        *handle.borrow_mut() = Some(
                            fs::OpenOptions::new()
                                .read(true)
                                .share_mode(1)
                                .open(journal.path().join("manifest.json"))?,
                        );
                        cc.set(cc.get() + 1);
                    }
                    Ok(())
                },
                || save_document(&mut rt, &mut session, context(&snap), &input).unwrap(),
            )
        },
    );
    assert_eq!(
        (c.calls, c.allocations, commits.get(), cleanup_count.get()),
        (1, 1, 1, 1)
    );
    let diag = result.execution.diagnostic();
    assert_eq!(diag.disk, DiskState::Committed);
    assert!(diag.recovery_required);
    assert_eq!(
        diag.artifact_commit.as_ref().unwrap().state,
        CommitResultState::CommittedCleanupFailed
    );
    assert_eq!(diag.artifact_commit.unwrap().io.unwrap().os_code, Some(32));
    candidate(&result, &expected);
    let candidate_ptr = result.outcome().unwrap() as *const _;
    let warning_ptr = result.outcome().unwrap().warnings().as_slice().as_ptr();
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = result.execution.body()
    else {
        panic!("committed cleanup result")
    };
    let crate::data::transaction::CommitOutcome::CommittedCleanupFailed {
        failure,
        cleanup_failure,
    } = outcome.implementation_original()
    else {
        panic!("original cleanup result")
    };
    let crate::data::transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
        panic!("actual OS cleanup cause")
    };
    let io_ptr = io as *const io::Error;
    assert_eq!(
        crate::data::transaction::artifact_diagnostics::IoDiagnostic::new(io).os_code,
        Some(32)
    );
    assert!(cleanup_failure.is_none());
    assert_eq!(rt.snapshot().state, RuntimeState::Pending);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    assert_eq!(journals(&f), 1);
    assert!(rt.ready().is_err());
    assert!(fs::read(document_path(&f, did)).unwrap() == expected);
    let new_disk = disk(&document_path(&f, did));
    let (blocked, c, k) =
        observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
    no_io(c, k);
    assert_eq!(
        blocked.execution.diagnostic().category,
        Some(ApplicationCategory::RuntimeRejected)
    );
    assert!(blocked.outcome().is_none());
    assert_disk(&document_path(&f, did), &new_disk);
    assert_eq!(journals(&f), 1);
    proof.assert(&input, &payload);
    held.borrow_mut().take();
    rt.recover().unwrap();
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(journals(&f), 0);
    assert_disk(&document_path(&f, did), &new_disk);
    // 복구가 과거 요청을 재발행하거나 stale token을 승격시키지 않는다.
    let (stale, c, k) =
        observe(|| save_document(&mut rt, &mut session, context(&snap), &input).unwrap());
    no_io(c, k);
    assert!(stale.outcome().is_none());
    let Some(BodyOutcome::Rejected(error)) = stale.execution.body() else {
        panic!("old source remains stale after recovery")
    };
    assert!(matches!(
        error.domain_cause(),
        Some(DocumentUpdateError::DocumentSourceMismatch)
    ));
    let fresh = save_input(&mut rt, tid, did, vec![]);
    assert!(fresh.document.token != input.document.token);
    let (unchanged, c, k) =
        observe(|| save_document(&mut rt, &mut session, context(&snap), &fresh).unwrap());
    no_io(c, k);
    assert!(matches!(
        unchanged.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    assert_warning(unchanged.outcome().unwrap().warnings());
    assert_disk(&document_path(&f, did), &new_disk);
    assert_disk(&template_path(&f, tid), &tb);
    candidate(&result, &expected);
    assert!(
        std::ptr::eq(result.outcome().unwrap(), candidate_ptr)
            && result.outcome().unwrap().warnings().as_slice().as_ptr() == warning_ptr
    );
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(retained)),
        ..
    }) = result.execution.body()
    else {
        panic!("retained cleanup owner")
    };
    let failure = retained.implementation_original().failures().unwrap().0;
    let crate::data::transaction::CommitFailureSource::Io(retained_io) = failure.source.as_ref()
    else {
        panic!("retained cleanup I/O")
    };
    assert!(std::ptr::eq(retained_io, io_ptr));
    proof.assert(&input, &payload);
    proof.drop_payload(payload);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    session.end_edit().unwrap();
    rt.close().unwrap();
}
