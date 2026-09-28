use super::*;
use crate::data::{
    collaboration_lock::LockSetReleaseError, edit_session::EditSessionErrorCategory,
    project_relative_path::ProjectRelativePath,
};

#[derive(Default)]
pub(super) struct ReleaseState {
    pub(super) acquire_count: usize,
    pub(super) fail_acquire: Option<usize>,
    pub(super) responses: VecDeque<Option<LockErrorCategory>>,
    pub(super) calls: Vec<(ProjectRelativePath, LockSessionId, usize, thread::ThreadId)>,
    pub(super) release_gate: Option<(usize, Gate)>,
}
pub(super) type ReleaseOwner = Arc<Mutex<ReleaseState>>;

// 기존 provider가 만든 실제 handle을 그대로 사용한다. 실패 응답만 요청별로 제어한다.
struct ReleaseProbe {
    inner: Probe,
    owner: ReleaseOwner,
}
impl LockService for ReleaseProbe {
    fn provider_info(&self) -> LockProviderInfo {
        self.inner.provider_info()
    }
    fn acquire(&self, r: LockAcquireRequest<'_>) -> Result<Box<dyn HeldLock>, LockError> {
        let mut s = self.owner.lock().unwrap();
        s.acquire_count += 1;
        if s.fail_acquire == Some(s.acquire_count) {
            return Err(LockError::for_request(
                LockErrorCategory::LockAcquireFailed,
                LockProviderKind::None,
                LockOperation::Acquire,
                &r,
            ));
        }
        drop(s);
        self.inner.acquire(r)
    }
    fn validate(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        self.inner.validate(held)
    }
    fn release(&self, held: &mut dyn HeldLock) -> Result<(), LockError> {
        let mut s = self.owner.lock().unwrap();
        s.calls.push((
            held.target().clone(),
            held.session_id().clone(),
            held as *mut dyn HeldLock as *mut () as usize,
            thread::current().id(),
        ));
        let response = s.responses.pop_front().expect("planned release pass");
        let gate = if s
            .release_gate
            .as_ref()
            .is_some_and(|(n, _)| *n == s.calls.len())
        {
            s.release_gate.take().map(|(_, gate)| gate)
        } else {
            None
        };
        drop(s);
        if let Some(gate) = gate {
            gate.block();
        }
        match response {
            Some(category) => Err(LockError::for_held(
                category,
                LockProviderKind::None,
                LockOperation::Release,
                held.project_fingerprint(),
                held.session_id(),
                held.target(),
            )),
            None => self.inner.release(held),
        }
    }
}
pub(super) fn release_owner(responses: Vec<Option<LockErrorCategory>>) -> ReleaseOwner {
    Arc::new(Mutex::new(ReleaseState {
        responses: responses.into(),
        ..ReleaseState::default()
    }))
}
pub(super) fn begin_release(
    ctx: &mut WorkerContext<Payload, Receipt>,
    (release, probe, targets): (ReleaseOwner, ProbeOwner, Vec<ProjectRelativePath>),
) -> context::SessionRegistration {
    ctx.begin_session(
        Arc::new(ReleaseProbe {
            inner: Probe::new(probe),
            owner: release,
        }),
        targets,
    )
    .unwrap()
}
pub(super) fn targets() -> Vec<ProjectRelativePath> {
    vec![ArtifactSourceId::Document(fixture::did()).path().unwrap()]
}
fn retry(c: &WorkerControl<Payload, Receipt>, reg: &Registration) -> context::SessionReleaseRetry {
    result(
        c,
        &c.try_cleanup(reg.clone(), |ctx, reg| {
            ctx.retry_session_release(&reg).unwrap()
        })
        .unwrap(),
    )
}
pub(super) fn end_failed(
    c: &WorkerControl<Payload, Receipt>,
    reg: &Registration,
) -> (EditSessionError, context::SessionReleaseObservation) {
    result(
        c,
        &c.try_cleanup(reg.clone(), |ctx, reg| {
            let original = ctx.end_session(&reg).unwrap().unwrap_err();
            assert_eq!(original.operation(), EditSessionOperation::EndEdit);
            let observed = ctx.session_release_observation(&reg).unwrap();
            assert_eq!(observed.snapshot.state(), EditSessionState::ReleaseFailed);
            (original, observed)
        })
        .unwrap(),
    )
}
pub(super) fn same_error(a: &LockError, b: &LockError) {
    assert_eq!(a.category(), b.category());
    assert_eq!(a.operation, b.operation);
    assert_eq!(a.provider, b.provider);
    assert!(a.target() == b.target());
    assert!(a.session_id() == b.session_id());
    assert!(a.project_fingerprint() == b.project_fingerprint());
    assert!(a.owner() == b.owner());
}
pub(super) fn release_error(error: &EditSessionError) -> &LockSetReleaseError {
    match error {
        EditSessionError::Release { source, .. } => source,
        _ => panic!("expected original release error"),
    }
}
pub(super) fn check_calls(
    owner: &ReleaseOwner,
    c: &WorkerControl<Payload, Receipt>,
    expected: usize,
) {
    let s = owner.lock().unwrap();
    assert_eq!(s.calls.len(), expected);
    assert!(s
        .calls
        .iter()
        .all(|call| Some(call.3) == c.snapshot().thread));
    assert!(s.calls.iter().all(|call| call.3 != thread::current().id()));
    for call in &s.calls {
        let first = s.calls.iter().find(|other| other.0 == call.0).unwrap();
        assert!(first.1 == call.1 && first.2 == call.2);
    }
}

#[test]
fn fix_r1_active_release_retry_preserves_original_and_rejects_invalid_owners() {
    let (_f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let (_other_f, mut other, other_client) = start::<Payload, Receipt>(2, 1);
    let foreign = result(
        &other,
        &other_client
            .try_submit(ProbeOwner::default(), register)
            .unwrap(),
    );
    let owner = release_owner(vec![Some(LockErrorCategory::LockReleaseFailed), None]);
    let (p, proof) = payload();
    let reg = result(
        &c,
        &client
            .try_submit((owner.clone(), p), |ctx, (owner, p)| {
                let reg = begin_release(ctx, (owner, ProbeOwner::default(), targets()));
                assert!(reg.original.is_ok());
                ctx.session(reg.key.as_ref().unwrap())
                    .unwrap()
                    .bind_draft(p)
                    .unwrap();
                reg
            })
            .unwrap(),
    );
    let ordinary = saturate(&c, &client);
    c.close_admission();
    // Editing도 하위 retry 계약대로 거부한다. 잘못된 등록으로 다른 owner를 찾지 않는다.
    let denied = retry(&c, &reg.registration);
    assert_eq!(
        denied.original.unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    assert_eq!(denied.before.snapshot, denied.after.snapshot);
    let mut wrong = reg.registration.clone();
    wrong.serial += 1;
    for invalid in [wrong, foreign.registration.clone()] {
        let t = c
            .try_cleanup(invalid, |ctx, r| ctx.retry_session_release(&r))
            .unwrap();
        assert_eq!(result(&c, &t).unwrap_err(), WorkerCategory::Stale);
    }
    check_calls(&owner, &c, 0);
    let (first, before) = end_failed(&c, &reg.registration);
    assert_eq!(first.category(), EditSessionErrorCategory::ReleaseFailed);
    same_error(
        &release_error(&first).errors[0],
        &before.release.as_ref().unwrap().first.errors[0],
    );
    let repeated = c
        .try_cleanup(reg.registration.clone(), |ctx, r| {
            ctx.end_session(&r).unwrap()
        })
        .unwrap();
    assert_eq!(
        result(&c, &repeated).unwrap_err().category(),
        EditSessionErrorCategory::InvalidState
    );
    check_calls(&owner, &c, 1);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
    let recovered = retry(&c, &reg.registration);
    recovered.original.unwrap();
    assert_eq!(recovered.before.snapshot, before.snapshot);
    same_error(
        &release_error(&first).errors[0],
        &recovered.before.release.unwrap().first.errors[0],
    );
    assert_eq!(recovered.after.snapshot.state(), EditSessionState::ReadOnly);
    assert!(recovered.after.snapshot.session_id().is_none());
    assert!(recovered.after.release.is_none());
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Ready);
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    let denied = retry(&c, &reg.registration);
    assert_eq!(
        denied.original.unwrap_err().operation(),
        EditSessionOperation::RetryRelease
    );
    check_calls(&owner, &c, 2);
    assert_eq!(owner.lock().unwrap().acquire_count, 1);
    let (denied_p, denied_proof) = payload();
    let closed = client.try_submit(denied_p, |_, p| p).unwrap_err();
    assert_eq!(closed.category, WorkerCategory::Closed);
    denied_proof.check(&closed.input);
    drop(closed);
    assert_eq!(denied_proof.drops.load(Ordering::SeqCst), 1);
    let t = c
        .try_cleanup(reg.registration.clone(), |ctx, r| {
            let p = ctx.return_active(&r).unwrap();
            ctx.remove_session(&r).unwrap();
            assert_eq!(
                ctx.retry_session_release(&r).unwrap_err(),
                WorkerCategory::Stale
            );
            p
        })
        .unwrap();
    let original = result(&c, &t);
    proof.check(&original);
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    check_calls(&owner, &c, 2);
    assert_eq!(c.snapshot().ordinary_outstanding, 2);
    assert_eq!(c.try_take::<u32>(&ordinary[0]).unwrap(), Some(19));
    assert_eq!(c.try_take::<u32>(&ordinary[1]).unwrap(), Some(23));
    drop((client, other_client));
    finish(&mut c, vec![]);
    finish(&mut other, vec![foreign.registration]);
}

#[test]
fn fix_r1_each_request_retries_only_remaining_handles_and_keeps_first_latest() {
    let (_f, mut c, client) = start::<Payload, Receipt>(2, 1);
    // 역순 첫 target은 성공, 남은 target만 최초 실패와 retry 실패를 거쳐 성공한다.
    let owner = release_owner(vec![
        None,
        Some(LockErrorCategory::LockReleaseFailed),
        Some(LockErrorCategory::LockStateUnknown),
        None,
    ]);
    let mut two = targets();
    two.push(ProjectRelativePath::parse("templates/retry-target.json").unwrap());
    let reg = result(
        &c,
        &client
            .try_submit((owner.clone(), ProbeOwner::default(), two), begin_release)
            .unwrap(),
    );
    assert!(reg.original.is_ok());
    c.close_admission();
    let (original, first) = end_failed(&c, &reg.registration);
    let diagnostics = first.release.unwrap();
    assert_eq!(diagnostics.retry_attempts, 0);
    assert_eq!(diagnostics.failure_count, 1);
    check_calls(&owner, &c, 2);
    let failed = retry(&c, &reg.registration);
    let latest = failed.original.unwrap_err();
    assert_eq!(latest.operation(), EditSessionOperation::RetryRelease);
    let diagnostics = failed.after.release.unwrap();
    same_error(
        &release_error(&original).errors[0],
        &diagnostics.first.errors[0],
    );
    same_error(
        &release_error(&latest).errors[0],
        &diagnostics.latest.errors[0],
    );
    assert_eq!(
        diagnostics.first.errors[0].category(),
        LockErrorCategory::LockReleaseFailed
    );
    assert_eq!(
        diagnostics.latest.errors[0].category(),
        LockErrorCategory::LockStateUnknown
    );
    assert_eq!(diagnostics.retry_attempts, 1);
    assert_eq!(diagnostics.failure_count, 2);
    assert_eq!(failed.after.snapshot.release_failure_count(), 2);
    assert_eq!(
        failed.after.snapshot.state(),
        EditSessionState::ReleaseFailed
    );
    check_calls(&owner, &c, 3);
    let recovered = retry(&c, &reg.registration);
    recovered.original.unwrap();
    assert_eq!(recovered.before.release.unwrap().retry_attempts, 1);
    assert_eq!(recovered.after.snapshot.state(), EditSessionState::ReadOnly);
    check_calls(&owner, &c, 4);
    {
        let s = owner.lock().unwrap();
        assert_eq!(s.acquire_count, 2);
        assert!(s.responses.is_empty());
        assert!(s.calls[0].0 != s.calls[1].0);
        assert!(s.calls[1].0 == s.calls[2].0 && s.calls[2].0 == s.calls[3].0);
    }
    result(
        &c,
        &c.try_cleanup(reg.registration, |ctx, r| ctx.remove_session(&r).unwrap())
            .unwrap(),
    );
    drop(client);
    finish(&mut c, vec![]);
}

#[test]
fn fix_r1_partial_acquire_cleanup_uses_failed_registration_without_reacquire() {
    let (_f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let owner = release_owner(vec![Some(LockErrorCategory::LockReleaseFailed), None]);
    owner.lock().unwrap().fail_acquire = Some(2);
    let mut two = targets();
    two.push(ProjectRelativePath::parse("templates/retry-target.json").unwrap());
    let reg = result(
        &c,
        &client
            .try_submit((owner.clone(), ProbeOwner::default(), two), begin_release)
            .unwrap(),
    );
    let (acquire, cleanup) = match reg.original {
        Err(EditSessionError::AcquireCleanup {
            acquire_error,
            cleanup_error,
            ..
        }) => (acquire_error, cleanup_error),
        _ => panic!("expected real partial acquisition cleanup failure"),
    };
    assert_eq!(acquire.category(), LockErrorCategory::LockAcquireFailed);
    // 정상 SessionWork를 만들 필요 없이 실패한 begin이 돌려준 같은 등록으로 정리한다.
    c.close_admission();
    let recovered = retry(&c, &reg.registration);
    same_error(&acquire, recovered.before.acquire_error.as_ref().unwrap());
    same_error(
        &cleanup.errors[0],
        &recovered.before.release.as_ref().unwrap().first.errors[0],
    );
    assert_eq!(
        recovered.before.snapshot.state(),
        EditSessionState::ReleaseFailed
    );
    recovered.original.unwrap();
    assert_eq!(recovered.after.snapshot.state(), EditSessionState::ReadOnly);
    assert!(recovered.after.snapshot.session_id().is_none());
    check_calls(&owner, &c, 2);
    assert_eq!(owner.lock().unwrap().acquire_count, 2);
    result(
        &c,
        &c.try_cleanup(reg.registration, |ctx, r| ctx.remove_session(&r).unwrap())
            .unwrap(),
    );
    drop(client);
    finish(&mut c, vec![]);
}

#[test]
fn fix_r1_retry_preserves_pending_payload_or_receipt_and_pending_runtime() {
    for accepted_before_retry in [false, true] {
        let (f, mut c, client) = start::<Payload, Receipt>(2, 1);
        let owner = release_owner(vec![Some(LockErrorCategory::LockReleaseFailed), None]);
        let probe = ProbeOwner::default();
        let sink = SinkOwner::default();
        let (p, proof) = payload();
        let init = client
            .try_submit(
                (owner.clone(), probe.clone(), sink.clone(), p),
                |ctx, (owner, probe, sink, p)| {
                    fixture::add_historical(ctx.runtime.as_mut().unwrap());
                    let reg = begin_release(ctx, (owner, probe, targets()));
                    assert!(reg.original.is_ok());
                    let key = reg.key.as_ref().unwrap();
                    connect(ctx, (key.clone(), sink));
                    ctx.session(key).unwrap().bind_draft(p).unwrap();
                    let input = document_input(ctx, ());
                    // 기존 G2 전이를 fixture에서 만들고 보존·accept·retry는 공식 worker view로 실행한다.
                    ctx.runtime.as_mut().unwrap().invalidate_recovery();
                    (reg, input)
                },
            )
            .unwrap();
        let (reg, input) = result(&c, &init);
        let disk = fixture::pair(&f);
        let key = reg.key.as_ref().unwrap();
        probe.lock().unwrap().fail = true;
        client.trigger(key, TriggerReason::Foreground).unwrap();
        wait_validations(&c, key, 1);
        let failure = c.take_validation_failure(key).unwrap();
        assert_eq!(
            failure.preserve.unwrap().unwrap(),
            CustodyTransition::Preserved
        );
        let checked = client
            .try_submit((key.clone(), input), |ctx, (key, input)| {
                let saved = ctx.session(&key).unwrap().save_dirty_document(&input);
                assert!(matches!(
                    saved,
                    Err(HandoffError::Rejected(
                        crate::data::application::recovery_handoff::HandoffCategory::InvalidState
                    ))
                ));
            })
            .unwrap();
        result(&c, &checked);
        c.close_admission();
        if accepted_before_retry {
            result(
                &c,
                &c.try_cleanup(reg.registration.clone(), |ctx, r| {
                    ctx.accept(&r).unwrap().unwrap()
                })
                .unwrap(),
            );
        }
        let (_, first) = end_failed(&c, &reg.registration);
        let expected = if accepted_before_retry {
            RecoveryHandoffStatus::DurablyAccepted
        } else {
            RecoveryHandoffStatus::Pending
        };
        assert_eq!(first.snapshot.recovery_handoff(), Some(expected));
        let recovered = retry(&c, &reg.registration);
        recovered.original.unwrap();
        assert_eq!(recovered.before.snapshot.recovery_handoff(), Some(expected));
        assert_eq!(recovered.after.snapshot.recovery_handoff(), Some(expected));
        assert_eq!(
            recovered.after.snapshot.state(),
            EditSessionState::RecoveryRequired
        );
        assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Pending);
        assert_eq!(
            sink.lock().unwrap().calls,
            usize::from(accepted_before_retry)
        );
        assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
        assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 0);
        let denied = retry(&c, &reg.registration);
        assert_eq!(
            denied.original.unwrap_err().category(),
            EditSessionErrorCategory::InvalidState
        );
        check_calls(&owner, &c, 2);
        assert_eq!(owner.lock().unwrap().acquire_count, 1);
        let ack = result(
            &c,
            &c.try_cleanup(
                (reg.registration, accepted_before_retry),
                |ctx, (r, accepted)| {
                    assert_eq!(
                        ctx.return_active(&r).err().unwrap(),
                        WorkerCategory::OwnersRemain
                    );
                    assert_eq!(
                        ctx.remove_session(&r).unwrap_err(),
                        WorkerCategory::OwnersRemain
                    );
                    if !accepted {
                        ctx.accept(&r).unwrap().unwrap();
                    }
                    let ack = ctx.acknowledge(&r).unwrap().unwrap();
                    ctx.remove_session(&r).unwrap();
                    ack
                },
            )
            .unwrap(),
        );
        check_receipt(&ack, &sink);
        assert_eq!(
            ack.before.handoff,
            Some(RecoveryHandoffStatus::DurablyAccepted)
        );
        assert_eq!(ack.after.runtime, RuntimeState::Pending);
        assert!(!ack.after.normal_exit_allowed);
        assert!(ack.after.receipt_returned);
        assert_eq!(ack.after.session, EditSessionState::ReadOnly);
        assert_eq!(sink.lock().unwrap().calls, 1);
        drop(ack);
        assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 1);
        let original = sink.lock().unwrap().accepted.take().unwrap();
        proof.check(&original);
        drop(original);
        assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
        fixture::same_pair(&f, &disk);
        drop(client);
        finish(&mut c, vec![]);
    }
}

// 완료했지만 회수하지 않은 결과 두 개로 포화를 만든다. cleanup 전 공간을 비우지 않는다.
fn saturate(
    c: &WorkerControl<Payload, Receipt>,
    client: &WorkerClient<Payload, Receipt>,
) -> [RequestId; 2] {
    let first = client.try_submit(19_u32, |_, value| value).unwrap();
    let second = client.try_submit(23_u32, |_, value| value).unwrap();
    wait_for(c, |m| {
        m.results
            .values()
            .filter(|v| matches!(v, Stored::Complete(_)))
            .count()
            == 2
    });
    assert_eq!(c.snapshot().ordinary_outstanding, 2);
    let (p, proof) = payload();
    let denied = client.try_submit(p, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Full);
    proof.check(&denied.input);
    drop(denied);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    [first.id(), second.id()]
}
fn recover_control<O: Send + 'static>(
    c: &WorkerControl<Payload, Receipt>,
    id: &RequestId,
    ordinary: &[RequestId; 2],
) -> O {
    assert_eq!(c.snapshot().ordinary_outstanding, 2);
    let (p, proof) = payload();
    let calls = Arc::new(AtomicUsize::new(0));
    let denied = c
        .try_cleanup((p, calls.clone()), |_, (p, calls)| {
            calls.fetch_add(1, Ordering::SeqCst);
            p
        })
        .unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Full);
    proof.check(&denied.input.0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.try_take::<()>(id).unwrap_err(),
        WorkerCategory::ResultType
    );
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    let original: O = c.try_take(id).unwrap().unwrap();
    assert_eq!(
        c.try_take::<()>(id).unwrap_err(),
        WorkerCategory::UnknownRequest
    );
    assert_eq!(c.snapshot().ordinary_outstanding, 2);
    assert_eq!(c.try_take::<u32>(&ordinary[0]).unwrap(), Some(19));
    assert_eq!(c.try_take::<u32>(&ordinary[1]).unwrap(), Some(23));
    drop(denied);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    original
}

#[test]
fn fix_r2_saturated_cleanup_active_payload_survives_drop_wrong_type_and_duplicate() {
    let (_f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let (p, proof) = payload();
    let reg = result(
        &c,
        &client
            .try_submit(p, |ctx, p| {
                let reg = register(ctx, ProbeOwner::default());
                ctx.session(reg.key.as_ref().unwrap())
                    .unwrap()
                    .bind_draft(p)
                    .unwrap();
                reg
            })
            .unwrap(),
    );
    let ordinary = saturate(&c, &client);
    c.close_admission();
    let ticket = c
        .try_cleanup(reg.registration, |ctx, r| {
            ctx.end_session(&r).unwrap().unwrap();
            let p = ctx.return_active(&r).unwrap();
            ctx.remove_session(&r).unwrap();
            p
        })
        .unwrap();
    wait_for(&c, |m| {
        matches!(m.control_result, Some((_, Stored::Complete(_))))
    });
    let id = ticket.id();
    drop((ticket, client));
    assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
    let original: Payload = recover_control(&c, &id, &ordinary);
    proof.check(&original);
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    // 회수한 바로 그 정리 슬롯으로 runtime close를 실행하고 stop/join을 완료한다.
    finish(&mut c, vec![]);
}

#[test]
fn fix_r2_saturated_cleanup_real_ack_survives_drop_wrong_type_and_duplicate() {
    let (_f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let probe = ProbeOwner::default();
    let sink = SinkOwner::default();
    let (p, proof) = payload();
    let reg = result(
        &c,
        &client
            .try_submit((probe.clone(), sink.clone(), p), |ctx, (probe, sink, p)| {
                let reg = register(ctx, probe);
                let key = reg.key.as_ref().unwrap();
                connect(ctx, (key.clone(), sink));
                ctx.session(key).unwrap().bind_draft(p).unwrap();
                reg
            })
            .unwrap(),
    );
    let key = reg.key.as_ref().unwrap();
    probe.lock().unwrap().fail = true;
    client.trigger(key, TriggerReason::Foreground).unwrap();
    wait_validations(&c, key, 1);
    assert_eq!(
        c.take_validation_failure(key)
            .unwrap()
            .preserve
            .unwrap()
            .unwrap(),
        CustodyTransition::Preserved
    );
    let ordinary = saturate(&c, &client);
    c.close_admission();
    // 포화 상태를 유지한 cleanup 안에서 실제 G10 accept와 ack를 모두 실행한다.
    let ticket = c
        .try_cleanup(reg.registration, |ctx, r| {
            ctx.accept(&r).unwrap().unwrap();
            ctx.end_session(&r).unwrap().unwrap();
            let ack = ctx.acknowledge(&r).unwrap().unwrap();
            ctx.remove_session(&r).unwrap();
            ack
        })
        .unwrap();
    wait_for(&c, |m| {
        matches!(m.control_result, Some((_, Stored::Complete(_))))
    });
    let id = ticket.id();
    drop((ticket, client));
    assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 0);
    let ack: Acknowledged<Receipt> = recover_control(&c, &id, &ordinary);
    check_receipt(&ack, &sink);
    assert_eq!(
        ack.before.handoff,
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(ack.before.session, EditSessionState::RecoveryRequired);
    assert!(!ack.before.receipt_returned && !ack.before.normal_exit_allowed);
    assert_eq!(ack.after.session, EditSessionState::ReadOnly);
    assert!(ack.after.receipt_returned && ack.after.normal_exit_allowed);
    assert_eq!(ack.after.runtime, RuntimeState::Ready);
    assert_eq!(sink.lock().unwrap().calls, 1);
    drop(ack);
    assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 1);
    let original = sink.lock().unwrap().accepted.take().unwrap();
    proof.check(&original);
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    finish(&mut c, vec![]);
}
