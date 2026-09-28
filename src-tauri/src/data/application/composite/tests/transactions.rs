use super::*;
use crate::data::{
    collaboration_lock::{
        HeldLock, LockAcquireRequest, LockError, LockErrorCategory, LockOperation,
        LockProviderInfo, LockService,
    },
    repository::{ArtifactWriteCategory, ArtifactWriteStage},
    storage_estimate::test_support::{with_storage_response, TestStorageResponse},
    transaction::{self, CommitResultState, PrepareFailPoint, PrepareStage},
};
use std::{cell::RefCell, io, os::windows::fs::OpenOptionsExt};

#[derive(Debug)]
struct OwnedCause(Arc<()>);
impl std::fmt::Display for OwnedCause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("G9_PRIVATE_IO_CAUSE")
    }
}
impl std::error::Error for OwnedCause {}
fn journals(f: &Fixture) -> usize {
    let p = f.root.join(".worldbuild/transactions");
    if !p.exists() {
        0
    } else {
        fs::read_dir(p)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len()
    }
}
fn original_owner(io: &io::Error, owner: &Arc<()>) {
    let cause = transaction::io_cause(io)
        .get_ref()
        .unwrap()
        .downcast_ref::<OwnedCause>()
        .unwrap();
    assert!(Arc::ptr_eq(&cause.0, owner));
    assert_eq!(Arc::strong_count(owner), 2);
}
fn cleanup_io(result: &CompositeSaveExecution) -> &io::Error {
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = result.execution.body()
    else {
        panic!("retained commit owner")
    };
    let transaction::CommitOutcome::CommittedCleanupFailed { failure, .. } =
        outcome.implementation_original()
    else {
        panic!("retained cleanup owner")
    };
    let transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
        panic!("retained original I/O owner")
    };
    io
}
fn safe(result: &CompositeSaveExecution) {
    let debug = format!("{result:?}");
    assert!(!debug.contains(PRIVATE));
    assert!(!debug.contains("G9_PRIVATE_IO_CAUSE"));
    assert!(!debug.contains("1E100") && !debug.contains("historical option"));
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
fn g9_two_replace_provider_order_and_pre_body_or_prepare_loss_keep_input_and_outcomes() {
    let mut prepare_validation = None;
    for case in ["normal", "before-body", "prepare", "empty-before-body"] {
        let (f, mut rt) = seeded_historical();
        let input = load_input(
            &mut rt,
            required_intent(),
            if case == "empty-before-body" {
                vec![]
            } else {
                vec![filled_edit()]
            },
        );
        let payload = Payload::new();
        let proof = InputProof::capture(&input, &payload);
        let before = pair(&f);
        let request = CompositeWriteRequest::new(&input).unwrap();
        let provider = Arc::new(ObservedProvider {
            inner: NoLockService::new(),
            calls: AtomicUsize::new(0),
            fail: AtomicUsize::new(0),
        });
        let mut session = Session::new(provider.clone());
        session
            .begin_edit(rt.project_fingerprint(), request.session_targets().to_vec())
            .unwrap();
        provider.calls.store(0, Ordering::SeqCst);
        provider.fail.store(
            match case {
                "before-body" | "empty-before-body" => 1,
                "prepare" => prepare_validation.unwrap(),
                _ => 0,
            },
            Ordering::SeqCst,
        );
        let reads = Rc::new(RefCell::new(Vec::new()));
        let read_count = reads.clone();
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
                Box::new(|_| panic!("Replace cannot create namespace")),
            )),
            false,
            || {
                with_canonical_prepare_hooks(
                    move |point, op| {
                        if point == PrepareFailPoint::BeforeOriginalRead {
                            read_count
                                .borrow_mut()
                                .push((op, p.calls.load(Ordering::SeqCst)));
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
                                update_template_and_save_document(
                                    &mut rt,
                                    &mut session,
                                    context(&snap),
                                    &request,
                                )
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
            let observed = reads.borrow();
            assert_eq!(
                observed.iter().map(|v| v.0).collect::<Vec<_>>(),
                vec![Some(0), Some(1)]
            );
            assert_eq!(
                (
                    observed[0].1,
                    observed[1].1,
                    at_commit.get(),
                    provider.calls.load(Ordering::SeqCst)
                ),
                (4, 4, 6, 12)
            );
            assert!(provider.calls.load(Ordering::SeqCst) > at_commit.get());
            prepare_validation = Some(observed[0].1);
            println!(
                "g9-two-replace-provider originals={:?} commit={} total={}",
                *observed,
                at_commit.get(),
                provider.calls.load(Ordering::SeqCst)
            );
            let (tb, db) = candidate_pair(&result);
            let actual = pair(&f);
            assert!(actual.template.0 == tb && actual.document.0 == db);
            assert!(actual.sentinel == before.sentinel);
        } else {
            assert_eq!(
                result.execution.diagnostic().permit.unwrap().lock_category,
                Some(LockErrorCategory::LockLost)
            );
            assert_eq!(session.snapshot().state(), EditSessionState::LockLost);
            if case == "prepare" {
                assert_eq!((c.calls, c.allocations, commits.get()), (1, 0, 0));
                assert_eq!(hooks.decoded, 2);
                candidate_pair(&result);
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
                assert!(result.template_outcome().is_none() && result.document_outcome().is_none());
            }
            same_pair(&f, &before);
            let (again, c, k) = observe(|| {
                update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
            });
            no_io(c, k);
            assert_eq!(
                again.execution.diagnostic().category,
                Some(ApplicationCategory::InvalidSessionState)
            );
        }
        safe(&result);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(journals(&f), 0);
        proof.check(&input, &payload);
        proof.drop_payload(payload);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_space_and_clean_prepare_failure_keep_both_candidates_warning_and_original_io_owner() {
    for space in [true, false] {
        let (f, mut rt) = seeded_historical();
        let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        let before = pair(&f);
        let payload = Payload::new();
        let proof = InputProof::capture(&input, &payload);
        let request = CompositeWriteRequest::new(&input).unwrap();
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let owner = Arc::new(());
        let injected = owner.clone();
        let commits = Rc::new(Cell::new(0));
        let kc = commits.clone();
        let execute = || {
            with_canonical_prepare_hooks(
                move |point, _| {
                    if !space && point == PrepareFailPoint::StagedWrite {
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
                        || {
                            update_template_and_save_document(
                                &mut rt,
                                &mut session,
                                context(&snap),
                                &request,
                            )
                        },
                    )
                },
            )
        };
        let (result, c) = if space {
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
            (1, usize::from(!space), 0)
        );
        assert_eq!(result.execution.diagnostic().disk, DiskState::NotApplied);
        candidate_pair(&result);
        safe(&result);
        let tp = result.template_outcome().unwrap() as *const _;
        let dp = result.document_outcome().unwrap() as *const _;
        let wp = result
            .document_outcome()
            .unwrap()
            .warnings()
            .as_slice()
            .as_ptr();
        let Some(BodyOutcome::Write {
            transaction: TransactionResult::PrepareFailed(error),
            ..
        }) = result.execution.body()
        else {
            panic!("original prepare failure")
        };
        if space {
            assert_eq!(
                result
                    .execution
                    .diagnostic()
                    .artifact_write
                    .unwrap()
                    .category,
                ArtifactWriteCategory::StorageRejected
            );
        } else {
            let io = error
                .implementation_original_prepare()
                .unwrap()
                .implementation_original_io()
                .unwrap();
            original_owner(io, &owner);
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
        }
        same_pair(&f, &before);
        assert_eq!(journals(&f), 0);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        assert!(
            std::ptr::eq(result.template_outcome().unwrap(), tp)
                && std::ptr::eq(result.document_outcome().unwrap(), dp)
                && result
                    .document_outcome()
                    .unwrap()
                    .warnings()
                    .as_slice()
                    .as_ptr()
                    == wp
        );
        proof.check(&input, &payload);
        proof.drop_payload(payload);
        drop(result);
        assert_eq!(Arc::strong_count(&owner), 1);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_each_original_token_detects_late_raw_race_without_overwriting_either_source() {
    for template in [false, true] {
        let (f, mut rt) = seeded_historical();
        let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
        let before = pair(&f);
        let payload = Payload::new();
        let proof = InputProof::capture(&input, &payload);
        let request = CompositeWriteRequest::new(&input).unwrap();
        assert!(request.session_targets()[0] < request.session_targets()[1]);
        let mut session = begin(&rt, request.session_targets());
        let snap = session.snapshot();
        let path = if template {
            f.template_path()
        } else {
            f.document_path()
        };
        let raced = Rc::new(RefCell::new(None));
        let rc = raced.clone();
        let hits = Rc::new(Cell::new(0));
        let hc = hits.clone();
        let commits = Rc::new(Cell::new(0));
        let kc = commits.clone();
        let (result, c) = with_canonical_prepare_hooks(
            move |point, op| {
                if point == PrepareFailPoint::BeforeOriginalRead && op == Some(u32::from(template))
                {
                    let mut bytes = fs::read(&path)?;
                    bytes.push(b' ');
                    fs::write(&path, &bytes)?;
                    *rc.borrow_mut() = Some(disk(&path));
                    hc.set(hc.get() + 1);
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
                    || {
                        update_template_and_save_document(
                            &mut rt,
                            &mut session,
                            context(&snap),
                            &request,
                        )
                    },
                )
            },
        );
        assert_eq!(
            (c.calls, c.allocations, commits.get(), hits.get()),
            (1, 1, 0, 1)
        );
        assert_eq!(result.execution.diagnostic().disk, DiskState::NotApplied);
        candidate_pair(&result);
        let diag = result.execution.diagnostic().artifact_write.unwrap();
        assert_eq!(
            (diag.category, diag.stage, diag.prepare_stage),
            (
                ArtifactWriteCategory::SourceMismatch,
                ArtifactWriteStage::ReadSource,
                Some(PrepareStage::ReadOriginal)
            )
        );
        assert_eq!(
            diag.target,
            Some(if template {
                ArtifactSourceId::Template(tid())
            } else {
                ArtifactSourceId::Document(did())
            })
        );
        let actual = pair(&f);
        let raced = raced.borrow();
        let raced = raced.as_ref().unwrap();
        if template {
            assert!(actual.template == *raced && actual.template.0 != before.template.0);
            assert!(actual.document == before.document);
        } else {
            assert!(actual.document == *raced && actual.document.0 != before.document.0);
            assert!(actual.template == before.template);
        }
        assert!(actual.sentinel == before.sentinel);
        assert_eq!(journals(&f), 0);
        assert_eq!(rt.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        proof.check(&input, &payload);
        proof.drop_payload(payload);
        session.end_edit().unwrap();
        rt.close().unwrap();
    }
}

#[test]
fn g9_actual_sorted_first_document_new_second_template_old_rolls_back_both_with_original_cause() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let before = pair(&f);
    let payload = Payload::new();
    let proof = InputProof::capture(&input, &payload);
    let request = CompositeWriteRequest::new(&input).unwrap();
    assert_eq!(
        request.session_targets()[0],
        ArtifactSourceId::Document(did()).path().unwrap()
    );
    assert_eq!(
        request.session_targets()[1],
        ArtifactSourceId::Template(tid()).path().unwrap()
    );
    let mut session = begin(&rt, request.session_targets());
    let snap = session.snapshot();
    let owner = Arc::new(());
    let injected = owner.clone();
    let commits = Rc::new(Cell::new(0));
    let kc = commits.clone();
    let middle = Rc::new(RefCell::new(None));
    let mc = middle.clone();
    let dp = f.document_path();
    let tp = f.template_path();
    let old = before.clone();
    let (result, c) = with_canonical_prepare_hooks(
        |_, _| Ok(()),
        || {
            with_commit_io_factory(
                move |point, _| {
                    if point == Some(CommitTestPoint::ManifestRevalidation) {
                        kc.set(kc.get() + 1);
                    }
                    // 정렬된 첫 entry의 handle이 닫힌 직후 실제 new/old를 읽고 기존 seam에서 실패시킨다.
                    if point == Some(CommitTestPoint::ProgressState) {
                        let doc = fs::read(&dp)?;
                        let template = fs::read(&tp)?;
                        assert!(
                            doc != old.document.0 && template == old.template.0,
                            "actual sorted first entry new, second still old"
                        );
                        let decoded = artifact::decode_document(&doc).unwrap();
                        assert_eq!(decoded.template_revision(), revision(5));
                        assert!(decoded.updated_at_utc() == SAVE);
                        super::owner(
                            &doc,
                            &["fieldValues", &key(3)],
                            &json!({"kind":"text","value":"filled"}),
                        );
                        assert!(mc.borrow().is_none());
                        *mc.borrow_mut() = Some((template, doc));
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            OwnedCause(injected.clone()),
                        ));
                    }
                    Ok(())
                },
                || {
                    update_template_and_save_document(
                        &mut rt,
                        &mut session,
                        context(&snap),
                        &request,
                    )
                },
            )
        },
    );
    assert_eq!((c.calls, c.allocations, commits.get()), (1, 1, 1));
    let (tb, db) = candidate_pair(&result);
    let observed = middle.borrow();
    let observed = observed
        .as_ref()
        .expect("actual intermediate disk observation");
    assert!(observed.1 == db && observed.0 == before.template.0 && tb != observed.0);
    assert_eq!(result.execution.diagnostic().disk, DiskState::RolledBack);
    assert_eq!(
        result.execution.diagnostic().artifact_commit.unwrap().state,
        CommitResultState::RolledBack
    );
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = result.execution.body()
    else {
        panic!("confirmed rollback outcome")
    };
    let (failure, cleanup) = outcome
        .implementation_original()
        .rollback_failures()
        .unwrap();
    assert!(cleanup.is_none());
    let transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
        panic!("original failure")
    };
    original_owner(io, &owner);
    safe(&result);
    same_old_bytes(&f, &before);
    assert_eq!(journals(&f), 0);
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    proof.check(&input, &payload);
    proof.drop_payload(payload);
    drop(result);
    assert_eq!(Arc::strong_count(&owner), 1);
    session.end_edit().unwrap();
    rt.close().unwrap();
}

#[test]
fn g9_real_cleanup_os32_keeps_both_new_owners_pending_then_recovers_and_requires_fresh_pair() {
    let (f, mut rt) = seeded_historical();
    let input = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    let before = pair(&f);
    let payload = Payload::new();
    let proof = InputProof::capture(&input, &payload);
    let request = CompositeWriteRequest::new(&input).unwrap();
    let mut session = begin(&rt, request.session_targets());
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
                    // 실제 committed marker를 확인한 뒤 delete sharing을 막아 OS cleanup 오류를 만든다.
                    if point == Some(CommitTestPoint::Cleanup) {
                        let entries = fs::read_dir(&root)?.collect::<Result<Vec<_>, _>>()?;
                        assert_eq!(entries.len(), 1);
                        let journal = entries[0].path();
                        assert!(journal.join("committed.json").exists());
                        *handle.borrow_mut() = Some(
                            fs::OpenOptions::new()
                                .read(true)
                                .share_mode(1)
                                .open(journal.join("manifest.json"))?,
                        );
                        cc.set(cc.get() + 1);
                    }
                    Ok(())
                },
                || {
                    update_template_and_save_document(
                        &mut rt,
                        &mut session,
                        context(&snap),
                        &request,
                    )
                },
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
    let (tb, db) = candidate_pair(&result);
    let actual = pair(&f);
    assert!(
        actual.template.0 == tb && actual.document.0 == db && actual.sentinel == before.sentinel
    );
    let tp = result.template_outcome().unwrap() as *const _;
    let dp = result.document_outcome().unwrap() as *const _;
    let wp = result
        .document_outcome()
        .unwrap()
        .warnings()
        .as_slice()
        .as_ptr();
    let Some(BodyOutcome::Write {
        transaction: TransactionResult::Commit(Ok(outcome)),
        ..
    }) = result.execution.body()
    else {
        panic!("committed cleanup outcome")
    };
    let transaction::CommitOutcome::CommittedCleanupFailed {
        failure,
        cleanup_failure,
    } = outcome.implementation_original()
    else {
        panic!("original cleanup result")
    };
    let transaction::CommitFailureSource::Io(io) = failure.source.as_ref() else {
        panic!("actual OS cause")
    };
    let io_ptr = io as *const io::Error;
    assert_eq!(
        transaction::artifact_diagnostics::IoDiagnostic::new(io).os_code,
        Some(32)
    );
    assert!(cleanup_failure.is_none());
    assert_eq!(rt.snapshot().state, RuntimeState::Pending);
    assert_eq!(session.snapshot().state(), EditSessionState::Editing);
    assert_eq!(journals(&f), 1);
    assert!(rt.ready().is_err());
    let (blocked, c, k) = observe(|| {
        update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
    });
    no_io(c, k);
    assert_eq!(
        blocked.execution.diagnostic().category,
        Some(ApplicationCategory::RuntimeRejected)
    );
    assert!(blocked.template_outcome().is_none() && blocked.document_outcome().is_none());
    same_pair(&f, &actual);
    assert_eq!(journals(&f), 1);
    proof.check(&input, &payload);
    held.borrow_mut().take();
    rt.recover().unwrap();
    assert_eq!(rt.snapshot().state, RuntimeState::Ready);
    assert_eq!(journals(&f), 0);
    same_pair(&f, &actual);
    let (stale, c, k) = observe(|| {
        update_template_and_save_document(&mut rt, &mut session, context(&snap), &request)
    });
    no_io(c, k);
    assert!(matches!(
        domain(&stale),
        CompositeSaveError::Source(DocumentUpdateError::DocumentSourceMismatch)
    ));
    assert!(stale.template_outcome().is_none() && stale.document_outcome().is_none());
    let mut fresh = load_input(&mut rt, required_intent(), vec![filled_edit()]);
    fresh.timestamp_utc = AGAIN.into();
    assert!(
        fresh.template.token != input.template.token
            && fresh.document.token != input.document.token
    );
    let fresh_request = CompositeWriteRequest::new(&fresh).unwrap();
    let (unchanged, c, k) = observe(|| {
        update_template_and_save_document(&mut rt, &mut session, context(&snap), &fresh_request)
    });
    no_io(c, k);
    assert!(matches!(
        unchanged.execution.body(),
        Some(BodyOutcome::NoWrite(()))
    ));
    assert!(matches!(
        unchanged.template_outcome(),
        Some(TemplateMutationOutcome::Unchanged)
    ));
    assert_eq!(
        unchanged.document_outcome().unwrap().kind(),
        DocumentSaveOutcomeKind::Unchanged
    );
    warning(unchanged.document_outcome().unwrap());
    same_pair(&f, &actual);
    candidate_pair(&result);
    safe(&result);
    assert!(
        std::ptr::eq(result.template_outcome().unwrap(), tp)
            && std::ptr::eq(result.document_outcome().unwrap(), dp)
            && result
                .document_outcome()
                .unwrap()
                .warnings()
                .as_slice()
                .as_ptr()
                == wp
    );
    assert!(std::ptr::eq(cleanup_io(&result), io_ptr));
    proof.check(&input, &payload);
    proof.drop_payload(payload);
    session.end_edit().unwrap();
    rt.close().unwrap();
}
