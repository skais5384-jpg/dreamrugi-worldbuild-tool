use super::*;
use crate::data::application::recovery_handoff::{HandoffCategory, SinkCapability};

fn waiting(c: &WorkerControl<Payload, Receipt>, reg: &Registration, key: &SessionKey) {
    let view = c.shutdown_snapshot();
    let s = view
        .sessions
        .iter()
        .find(|s| s.owner.registration == *reg)
        .unwrap();
    assert!(s.registered && s.awaiting_custody && s.owner.release_pending);
    assert_eq!(s.owner.custody, Custody::Active);
    assert_eq!(s.owner.release.snapshot.state(), EditSessionState::Editing);
    assert!(s.owner.release.snapshot.session_id() == Some(&key.session));
    assert!(s.owner.release.snapshot.project_fingerprint() == Some(key.project.as_str()));
    assert!(s.owner.release.snapshot.targets() == targets());
    assert!(s.first_failure.is_none());
    assert_eq!((s.round_passes, s.total_passes), (0, 0));
    assert!(view.blockers.contains(&ShutdownBlocker::CustodyDecision));
    assert!(!view.normal_exit_allowed && !view.stop_requested);
}

// 감사의 정상 대조와 반례는 호출 순서만 바꾸고 같은 실제 P/R·복구·최종 계약을 검증한다.
fn active_handoff(preserve_before_shutdown: bool) {
    let (f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let (p, proof) = payload();
    let provider = ProbeOwner::default();
    let sink = SinkOwner::default();
    let reg = result(
        &c,
        &client
            .try_submit(
                (p, provider.clone(), sink.clone()),
                |ctx, (p, provider, sink)| {
                    let reg = register(ctx, provider);
                    ctx.session(reg.key.as_ref().unwrap())
                        .unwrap()
                        .bind_draft(p)
                        .unwrap();
                    connect(ctx, (reg.key.as_ref().unwrap().clone(), sink));
                    reg
                },
            )
            .unwrap(),
    );
    let obstacle = f.root.join(".worldbuild");
    assert!(!obstacle.exists());
    fs::write(&obstacle, b"recovery obstacle").unwrap();
    let original_error =
        result(&c, &client.try_submit((), |ctx, ()| ctx.recover()).unwrap()).unwrap_err();
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Blocked);
    c.close_admission();
    if preserve_before_shutdown {
        assert_eq!(
            result(
                &c,
                &c.try_cleanup(reg.registration.clone(), |ctx, r| ctx.preserve_existing(&r))
                    .unwrap()
            )
            .unwrap()
            .unwrap(),
            CustodyTransition::Preserved
        );
    }
    c.request_shutdown();
    if !preserve_before_shutdown {
        settled(&c);
        waiting(&c, &reg.registration, reg.key.as_ref().unwrap());
        assert!(c.take_shutdown_report().is_none());
        assert_eq!(count(&provider, "release"), 0);
        assert_eq!(sink.lock().unwrap().calls, 0);
        assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
        assert_eq!(
            result(
                &c,
                &c.try_cleanup(reg.registration.clone(), |ctx, r| ctx.preserve_existing(&r))
                    .unwrap()
            )
            .unwrap()
            .unwrap(),
            CustodyTransition::Preserved
        );
    }
    let ended = report(&c);
    assert_eq!(ended.sessions.len(), 1);
    assert_eq!(ended.sessions[0].registration, reg.registration);
    assert!(ended.sessions[0].end.as_ref().unwrap().original.is_ok());
    assert!(ended.sessions[0].retries.is_empty());
    // Pending은 accept/ack 전에도 하위 계약대로 release될 수 있다.
    let view = c.shutdown_snapshot();
    assert_eq!(view.sessions[0].owner.custody, Custody::Pending);
    assert!(!view.sessions[0].owner.release_pending && !view.sessions[0].awaiting_custody);
    assert_eq!(sink.lock().unwrap().calls, 0);
    let ack = result(
        &c,
        &c.try_cleanup(reg.registration.clone(), |ctx, r| {
            ctx.accept(&r).unwrap().unwrap();
            ctx.acknowledge(&r).unwrap().unwrap()
        })
        .unwrap(),
    );
    check_receipt(&ack, &sink);
    proof.check(sink.lock().unwrap().accepted.as_ref().unwrap());
    assert_eq!(
        ack.before.handoff,
        Some(RecoveryHandoffStatus::DurablyAccepted)
    );
    assert_eq!(ack.after.handoff, Some(RecoveryHandoffStatus::Acknowledged));
    drop(ack);
    assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 1);
    assert!(fs::read(&obstacle).unwrap() == b"recovery obstacle");
    fs::remove_file(&obstacle).unwrap();
    result(
        &c,
        &c.try_cleanup((), |ctx, ()| ctx.recover_runtime()).unwrap(),
    )
    .unwrap();
    complete(&mut c, true);
    assert_eq!(c.shutdown_snapshot().caller_custody, 0);
    assert_eq!(count(&provider, "release"), 1);
    assert_eq!(count(&provider, "acquire"), 1);
    assert_eq!(sink.lock().unwrap().calls, 1);
    no_secrets(&format!("{original_error:?}"), &f.root);
    drop((client, sink));
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn r1_preserve_before_shutdown_keeps_real_handoff() {
    active_handoff(true);
}

#[test]
fn r1_shutdown_before_preserve_keeps_real_handoff() {
    active_handoff(false);
}

#[test]
fn r1_wait_keeps_identity_on_rejected_preserve_and_other_registration_budget() {
    let (f, mut c, client) = start::<Payload, Receipt>(2, 2);
    let (p, proof) = payload();
    let provider = ProbeOwner::default();
    let reg = result(
        &c,
        &client
            .try_submit((p, provider.clone()), |ctx, (p, provider)| {
                let reg = register(ctx, provider);
                ctx.session(reg.key.as_ref().unwrap())
                    .unwrap()
                    .bind_draft(p)
                    .unwrap();
                reg
            })
            .unwrap(),
    );
    let failing = release_owner(
        vec![Some(LockErrorCategory::LockReleaseFailed); 3]
            .into_iter()
            .chain([None])
            .collect(),
    );
    let other = result(
        &c,
        &client
            .try_submit(
                (failing.clone(), ProbeOwner::default(), targets()),
                begin_release,
            )
            .unwrap(),
    );
    let obstacle = f.root.join(".worldbuild");
    fs::write(&obstacle, b"recovery obstacle").unwrap();
    result(&c, &client.try_submit((), |ctx, ()| ctx.recover()).unwrap()).unwrap_err();
    c.request_shutdown();
    let first = report(&c);
    assert_eq!(first.sessions.len(), 1);
    assert_eq!(first.sessions[0].registration, other.registration);
    assert!(first.sessions[0].end.as_ref().unwrap().original.is_err());
    assert_eq!(first.sessions[0].retries.len(), 2);
    check_calls(&failing, &c, 3);
    waiting(&c, &reg.registration, reg.key.as_ref().unwrap());
    assert_eq!(
        c.shutdown_snapshot().sessions[0].owner.capability,
        SinkCapability::Unconnected
    );
    for _ in 0..3 {
        c.request_shutdown();
        assert!(c.take_shutdown_report().is_none());
        waiting(&c, &reg.registration, reg.key.as_ref().unwrap());
        assert_eq!(count(&provider, "release"), 0);
        check_calls(&failing, &c, 3);
    }
    // 실제 recovery 뒤 같은 등록의 preserve가 거부되어도 원본 처리 결정은 미완료다.
    fs::remove_file(&obstacle).unwrap();
    let t = c
        .try_cleanup(reg.registration.clone(), |ctx, r| {
            ctx.recover_runtime().unwrap();
            let error = ctx.preserve_existing(&r).unwrap().unwrap_err();
            (error, ctx.session_release_observation(&r).unwrap())
        })
        .unwrap();
    let id = t.id();
    drop((t, client));
    wait_for(&c, |m| {
        matches!(m.control_result, Some((_, Stored::Complete(_))))
    });
    let (denied, observed): (HandoffError, context::SessionReleaseObservation) =
        c.try_take(&id).unwrap().unwrap();
    assert!(matches!(
        denied,
        HandoffError::Rejected(HandoffCategory::NoRecoveryCondition)
    ));
    assert!(observed.snapshot.session_id() == Some(&reg.key.as_ref().unwrap().session));
    settled(&c);
    waiting(&c, &reg.registration, reg.key.as_ref().unwrap());
    assert!(
        !c.shutdown_snapshot().sessions[0]
            .owner
            .can_preserve_existing
    );
    assert_eq!(count(&provider, "release"), 0);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
    assert_eq!(
        c.try_take::<()>(&id).unwrap_err(),
        WorkerCategory::UnknownRequest
    );
    fs::write(&obstacle, b"recovery obstacle").unwrap();
    assert_eq!(
        result(
            &c,
            &c.try_cleanup(reg.registration.clone(), |ctx, r| {
                ctx.recover_runtime().unwrap_err();
                ctx.preserve_existing(&r).unwrap().unwrap()
            })
            .unwrap()
        ),
        CustodyTransition::Preserved
    );
    let resumed = report(&c);
    assert_eq!(resumed.sessions.len(), 1);
    assert_eq!(resumed.sessions[0].registration, reg.registration);
    assert!(resumed.sessions[0].end.as_ref().unwrap().original.is_ok());
    assert!(resumed.sessions[0].retries.is_empty());
    assert_eq!(count(&provider, "release"), 1);
    check_calls(&failing, &c, 3);
    let sink = SinkOwner::default();
    sink.lock().unwrap().fail_next = true;
    let rejected = result(
        &c,
        &c.try_cleanup(
            (reg.registration.clone(), sink.clone()),
            |ctx, (r, sink)| {
                assert!(matches!(
                    ctx.accept(&r).unwrap(),
                    Err(HandoffError::Rejected(HandoffCategory::Unavailable))
                ));
                assert!(ctx
                    .connect_backend(
                        &r,
                        RecoveryBackend::connected(Sink {
                            owner: sink,
                            local_calls: Rc::new(Cell::new(0))
                        })
                    )
                    .is_ok());
                let rejected = ctx.accept(&r).unwrap().unwrap_err();
                assert_eq!(
                    ctx.session_release_observation(&r)
                        .unwrap()
                        .snapshot
                        .recovery_handoff(),
                    Some(RecoveryHandoffStatus::Pending)
                );
                rejected
            },
        )
        .unwrap(),
    );
    assert!(matches!(rejected, HandoffError::Accept(_)));
    assert_eq!(sink.lock().unwrap().seen_address, Some(proof.address));
    assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
    let ack = result(
        &c,
        &c.try_cleanup(reg.registration.clone(), |ctx, r| {
            ctx.accept(&r).unwrap().unwrap();
            ctx.acknowledge(&r).unwrap().unwrap()
        })
        .unwrap(),
    );
    check_receipt(&ack, &sink);
    proof.check(sink.lock().unwrap().accepted.as_ref().unwrap());
    drop(ack);
    assert_eq!(sink.lock().unwrap().receipt_drops.load(Ordering::SeqCst), 1);
    let before = c.shutdown_snapshot();
    let other_before = before
        .sessions
        .iter()
        .find(|s| s.owner.registration == other.registration)
        .unwrap();
    assert_eq!(
        (other_before.round_passes, other_before.total_passes),
        (2, 2)
    );
    c.request_release_round().unwrap();
    let retried = report(&c);
    assert_eq!(retried.sessions.len(), 1);
    assert_eq!(retried.sessions[0].registration, other.registration);
    assert!(retried.sessions[0].end.is_none());
    assert_eq!(retried.sessions[0].retries.len(), 1);
    assert!(retried.sessions[0].retries[0].original.is_ok());
    let after = c.shutdown_snapshot();
    let other_after = after
        .sessions
        .iter()
        .find(|s| s.owner.registration == other.registration)
        .unwrap();
    assert_eq!((other_after.round_passes, other_after.total_passes), (1, 3));
    same_error(
        &other_before
            .first_failure
            .as_ref()
            .unwrap()
            .release
            .as_ref()
            .unwrap()
            .first
            .errors[0],
        &other_after
            .first_failure
            .as_ref()
            .unwrap()
            .release
            .as_ref()
            .unwrap()
            .first
            .errors[0],
    );
    fs::remove_file(&obstacle).unwrap();
    result(
        &c,
        &c.try_cleanup((), |ctx, ()| ctx.recover_runtime()).unwrap(),
    )
    .unwrap();
    complete(&mut c, true);
    check_calls(&failing, &c, 4);
    assert_eq!(failing.lock().unwrap().acquire_count, 1);
    assert_eq!(count(&provider, "release"), 1);
    assert_eq!(sink.lock().unwrap().calls, 2);
    no_secrets(&format!("{denied:?} {rejected:?}"), &f.root);
    drop(sink);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
}

thread_local! {
    static CLOSE_CALLS: std::cell::RefCell<Option<Arc<AtomicUsize>>> = const { std::cell::RefCell::new(None) };
}

fn close_then_stop(stop_before_refresh: bool) {
    let (f, mut c, client) = start::<(), ()>(1, 1);
    let obstacle = f.root.join(".worldbuild");
    fs::write(&obstacle, b"close obstacle").unwrap();
    let recovery_error =
        result(&c, &client.try_submit((), |ctx, ()| ctx.recover()).unwrap()).unwrap_err();
    c.request_shutdown();
    settled(&c);
    assert!(c
        .shutdown_snapshot()
        .blockers
        .contains(&ShutdownBlocker::RuntimeRecovery));
    assert!(c.take_shutdown_report().is_none());
    fs::remove_file(&obstacle).unwrap();
    let (first_gate, first) = gate();
    let (second_gate, second) = gate();
    let calls = Arc::new(AtomicUsize::new(0));
    let t = c
        .try_cleanup(
            (first_gate, second_gate, calls.clone()),
            |ctx, (first_gate, second_gate, calls)| {
                CLOSE_CALLS.with(|s| *s.borrow_mut() = Some(calls));
                ctx.0.close_runtime_with = |runtime| {
                    CLOSE_CALLS
                        .with(|s| s.borrow().as_ref().unwrap().fetch_add(1, Ordering::SeqCst));
                    runtime.close()
                };
                ctx.recover_runtime().unwrap();
                let closed = ctx.close_runtime().unwrap();
                assert_eq!(
                    ctx.close_runtime().unwrap_err(),
                    WorkerCategory::Unavailable
                );
                // 결과 공개 후 / 재집계 후 gate. 원 결과 회수 전에 settled를 호출하지 않는다.
                super::super::AFTER_JOB
                    .with(|s| *s.borrow_mut() = [first_gate, second_gate].into());
                closed
            },
        )
        .unwrap();
    first.entered();
    assert_eq!(
        c.try_take::<()>(&t.id()).unwrap_err(),
        WorkerCategory::ResultType
    );
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    let closed: Result<
        crate::data::project_runtime::RuntimeSnapshot,
        Box<crate::data::project_runtime::RuntimeCloseError>,
    > = c.try_take(&t.id()).unwrap().unwrap();
    assert_eq!(closed.unwrap().state, RuntimeState::Ready);
    assert_eq!(
        c.try_take::<()>(&t.id()).unwrap_err(),
        WorkerCategory::UnknownRequest
    );
    assert!(c.snapshot().runtime.is_none());
    assert!(c.shutdown_snapshot().close.is_none());
    assert!(c
        .shutdown_snapshot()
        .blockers
        .contains(&ShutdownBlocker::RuntimeRecovery));
    if stop_before_refresh {
        // 재집계 전 stop을 실제로 호출하고 비차단 거부를 확인한다.
        assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
        assert!(!c.shutdown_snapshot().stop_requested);
        assert!(!c.try_join().unwrap());
    }
    first.release();
    second.entered();
    let refreshed = c.shutdown_snapshot();
    assert!(
        matches!(refreshed.close, Some(CloseObservation::Closed(s)) if s.state == RuntimeState::Ready)
    );
    assert!(refreshed.blockers.is_empty());
    assert!(c.take_shutdown_report().is_none());
    c.request_stop().unwrap();
    // 수락된 stop과 thread 종료 사이에도 새 cleanup owner를 몰래 받아 버리지 않는다.
    let (p, proof) = payload();
    let rejected = c.try_cleanup(p, |_, p| p).unwrap_err();
    assert_eq!(rejected.category, WorkerCategory::Closed);
    proof.check(&rejected.input);
    assert!(c.pending_cleanup_id().is_none());
    second.release();
    wait_for(&c, |m| m.status == WorkerStatus::Stopped);
    join(&mut c);
    let view = c.shutdown_snapshot();
    assert!(view.resources_complete && view.joined && view.normal_exit_allowed);
    assert!(view.blockers.is_empty() && view.caller_custody == 0);
    assert!(
        matches!(view.close, Some(CloseObservation::Closed(s)) if s.state == RuntimeState::Ready)
    );
    assert!(c.snapshot().sessions == 0 && c.snapshot().runtime.is_none());
    for _ in 0..3 {
        assert!(c.try_join().unwrap());
        assert!(c.request_shutdown().normal_exit_allowed);
        assert!(c.take_shutdown_report().is_none());
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    no_secrets(&format!("{recovery_error:?} {view:?}"), &f.root);
    drop((client, rejected));
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn r2_explicit_stop_before_refresh_preserves_ready_close_and_normal_exit() {
    close_then_stop(true);
}

#[test]
fn r2_refresh_before_stop_preserves_ready_close_and_normal_exit() {
    close_then_stop(false);
}
