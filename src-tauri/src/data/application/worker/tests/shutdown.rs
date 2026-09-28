use super::release_cleanup::{
    begin_release, check_calls, end_failed, release_error, release_owner, same_error, targets,
};
use super::*;

mod fix;
use crate::data::application::shutdown::{CloseObservation, Custody, ShutdownBlocker};
use crate::data::project_relative_path::ProjectRelativePath;

// channel/Condvar가 사건 순서를 정한다. LIMIT은 성공 조건이 아닌 hang 상한이다.
fn settled<P, R>(c: &WorkerControl<P, R>) {
    wait_for(c, |m| {
        !m.running
            && m.queue.is_empty()
            && (!m.shutdown.pending || m.shutdown.has_report() || m.control_result.is_some())
    });
}
fn report<P: 'static, R: 'static>(c: &WorkerControl<P, R>) -> ShutdownReport {
    wait_for(c, |m| m.shutdown.has_report() && !m.running);
    let report = c.take_shutdown_report().unwrap();
    settled(c);
    report
}
fn result<P: 'static, R: 'static, O: Send + 'static>(c: &WorkerControl<P, R>, t: &Ticket<O>) -> O {
    let original = super::result(c, t);
    // 회수가 깨운 종료 재집계와 다음 cleanup 제출 사이도 명시적 사건 경계로 정한다.
    settled(c);
    original
}
fn join<P: 'static, R: 'static>(c: &mut WorkerControl<P, R>) {
    let end = Instant::now() + LIMIT;
    while !c.try_join().unwrap() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
}
fn complete<P: 'static, R: 'static>(
    c: &mut WorkerControl<P, R>,
    normal: bool,
) -> Vec<ShutdownReport> {
    let mut reports = Vec::new();
    loop {
        wait_for(c, |m| {
            (m.shutdown.has_report() && !m.running) || m.status == WorkerStatus::Stopped
        });
        if let Some(report) = c.take_shutdown_report() {
            reports.push(report);
        }
        if c.snapshot().status == WorkerStatus::Stopped {
            break;
        }
    }
    let before = c.shutdown_snapshot();
    assert!(
        before.stop_requested
            && !before.joined
            && !before.resources_complete
            && !before.normal_exit_allowed
    );
    join(c);
    let view = c.shutdown_snapshot();
    assert_eq!(view.phase, ShutdownPhase::Joined);
    assert!(view.resources_complete && view.joined && !view.report_pending);
    assert_eq!(view.normal_exit_allowed, normal);
    assert!(c.snapshot().runtime.is_none() && c.snapshot().sessions == 0);
    assert_eq!(c.request_shutdown().phase, ShutdownPhase::Joined);
    assert!(c.take_shutdown_report().is_none());
    assert!(c.try_join().unwrap());
    reports
}
fn no_secrets(text: &str, root: &std::path::Path) {
    for secret in [
        SECRET,
        RECEIPT,
        "provider-token-sentinel",
        "credential=secret",
        "C:/Users/private-user",
        &root.to_string_lossy(),
    ] {
        assert!(!text.contains(secret), "safe diagnostic boundary");
    }
}

#[test]
fn s1_all_actual_registrations_empty_released_and_first_failure_end_before_retry() {
    let (_f, mut c, client) = start::<Payload, Receipt>(4, 5);
    let failure = release_owner(vec![Some(LockErrorCategory::LockReleaseFailed), None]);
    let first = result(
        &c,
        &client
            .try_submit(
                (failure.clone(), ProbeOwner::default(), targets()),
                begin_release,
            )
            .unwrap(),
    );
    let owner = ProbeOwner::default();
    let second = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let old = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let empty = result(
        &c,
        &client
            .try_submit((), |ctx, ()| {
                ctx.begin_session(Arc::new(NoLockService::new()), vec![])
                    .unwrap()
            })
            .unwrap(),
    );
    c.close_admission();
    result(
        &c,
        &c.try_cleanup(old.registration.clone(), |ctx, r| {
            ctx.end_session(&r).unwrap().unwrap();
        })
        .unwrap(),
    );
    result(
        &c,
        &c.try_cleanup(second.registration.clone(), |ctx, r| {
            assert_eq!(
                ctx.remove_session(&r).unwrap_err(),
                WorkerCategory::OwnersRemain
            );
            assert_eq!(
                ctx.close_runtime().unwrap_err(),
                WorkerCategory::OwnersRemain
            );
        })
        .unwrap(),
    );
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    c.request_shutdown();
    let initial = report(&c);
    assert_eq!(initial.sessions.len(), 2);
    assert_eq!(initial.sessions[0].registration, first.registration);
    assert!(initial.sessions[0].end.as_ref().unwrap().original.is_err());
    assert_eq!(initial.sessions[1].registration, second.registration);
    assert!(initial.sessions[1].end.as_ref().unwrap().original.is_ok());
    assert_eq!(initial.sessions[0].retries.len(), 1);
    let view = c.shutdown_snapshot();
    assert_eq!(view.sessions.len(), 4);
    for reg in [empty.registration, old.registration] {
        let entry = view
            .sessions
            .iter()
            .find(|s| s.owner.registration == reg)
            .unwrap();
        assert!(!entry.owner.release_pending);
        assert_eq!(entry.total_passes, 0);
    }
    check_calls(&failure, &c, 2);
    assert_eq!(count(&owner, "release"), 2);
    let final_reports = complete(&mut c, true);
    assert_eq!(
        final_reports.iter().filter(|r| r.close.is_some()).count(),
        1
    );
    assert!(
        matches!(c.shutdown_snapshot().close, Some(CloseObservation::Closed(s)) if s.state == RuntimeState::Ready)
    );
    assert!(owner
        .lock()
        .unwrap()
        .events
        .iter()
        .all(|(_, t)| Some(*t) == c.snapshot().thread));
    check_calls(&failure, &c, 2);
}

#[test]
fn s1_s8_zero_sessions_final_report_claim_then_real_stop_join_once() {
    let (_f, mut c, client) = start::<(), ()>(1, 1);
    c.request_shutdown();
    wait_for(&c, |m| m.shutdown.has_report() && !m.running);
    assert!(c.snapshot().runtime.is_none());
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    assert!(!c.try_join().unwrap());
    for _ in 0..10 {
        assert!(!c.request_shutdown().normal_exit_allowed);
    }
    drop(client);
    let reports = complete(&mut c, true);
    assert_eq!(reports.len(), 1);
    assert!(reports[0].close.as_ref().unwrap().is_ok());
    assert!(reports[0].sessions.is_empty());
}

#[test]
fn s2_accepted_running_follow_up_save_and_late_registration_drain_before_end() {
    let (f, mut c, client) = start::<Payload, Receipt>(3, 2);
    let owner = ProbeOwner::default();
    let (reg, input) = result(
        &c,
        &client
            .try_submit(owner.clone(), |ctx, owner| {
                fixture::add_historical(ctx.runtime.as_mut().unwrap());
                let reg = register(ctx, owner);
                (reg, document_input(ctx, ()))
            })
            .unwrap(),
    );
    let key = reg.key.unwrap();
    let (g, held) = gate();
    owner.lock().unwrap().block = Some(g);
    client.trigger(&key, TriggerReason::Foreground).unwrap();
    held.entered();
    assert_eq!(
        client.trigger(&key, TriggerReason::OsResume).unwrap(),
        TriggerAccepted::FollowUp
    );
    let save = client
        .try_submit(
            (key.clone(), input, owner.clone()),
            |ctx, (key, input, owner)| {
                assert_eq!(count(&owner, "release"), 0);
                let before_validations = count(&owner, "validate");
                let (saved, prepare, commits) =
                    fixture::observe(|| ctx.session(&key).unwrap().save_document(&input));
                (
                    saved,
                    prepare.calls,
                    commits,
                    before_validations,
                    count(&owner, "validate"),
                )
            },
        )
        .unwrap();
    let late = client
        .try_submit(owner.clone(), |ctx, owner| {
            assert_eq!(count(&owner, "release"), 0);
            register(ctx, owner)
        })
        .unwrap();
    assert_eq!(c.request_shutdown().phase, ShutdownPhase::Draining);
    let (p, proof) = payload();
    let denied = client.try_submit(p, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Closed);
    proof.check(&denied.input);
    assert_eq!(
        client.trigger(&key, TriggerReason::Foreground).unwrap_err(),
        WorkerCategory::Closed
    );
    assert_eq!(count(&owner, "release"), 0);
    assert!(!c.try_join().unwrap());
    drop(client);
    held.release();
    let ended = report(&c);
    assert_eq!(ended.sessions.len(), 2);
    assert_eq!(count(&owner, "validate"), 7);
    assert_eq!(count(&owner, "release"), 2);
    let (saved, prepare, commits, before_validations, after_validations) = result(&c, &save);
    assert_eq!((before_validations, after_validations), (1, 6));
    // 실제 저장의 다섯 validation과 예약된 follow-up 한 번을 각각 구분한다.
    assert_eq!(count(&owner, "validate"), after_validations + 1);
    let saved = saved.unwrap();
    assert_eq!((prepare, commits), (1, 1));
    assert_eq!(saved.execution.diagnostic().disk, DiskState::Committed);
    let outcome = saved.outcome().unwrap();
    assert_eq!(outcome.kind(), DocumentSaveOutcomeKind::Changed);
    assert!(fixture::pair(&f).document.0 == artifact::encode_document(outcome.document()).unwrap());
    let late_reg = result(&c, &late);
    assert!(ended
        .sessions
        .iter()
        .any(|r| r.registration == late_reg.registration));
    let events = &owner.lock().unwrap().events;
    let first_end = events.iter().position(|(n, _)| *n == "release").unwrap();
    assert!(events[first_end..].iter().all(|(n, _)| *n == "release"));
    drop(denied);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    complete(&mut c, true);
}

#[test]
fn s3_s9_two_targets_exact_calls_first_latest_success_pass_and_nonblocking_view() {
    let (f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let first = LockErrorCategory::LockReleaseFailed;
    let latest = LockErrorCategory::LockLost;
    let owner = release_owner(vec![Some(first), None, Some(latest), None]);
    let mut paths = targets();
    paths.push(ProjectRelativePath::parse("templates/other.json").unwrap());
    let reg = result(
        &c,
        &client
            .try_submit((owner.clone(), ProbeOwner::default(), paths), begin_release)
            .unwrap(),
    );
    let (g2, held2) = gate();
    owner.lock().unwrap().release_gate = Some((2, g2));
    c.request_shutdown();
    held2.entered();
    check_calls(&owner, &c, 2);
    assert_eq!(c.shutdown_snapshot().phase, ShutdownPhase::Ending);
    let (g3, held3) = gate();
    owner.lock().unwrap().release_gate = Some((3, g3));
    held2.release();
    held3.entered();
    check_calls(&owner, &c, 3);
    let view = c.shutdown_snapshot();
    assert_eq!(view.phase, ShutdownPhase::Retrying);
    assert_eq!(
        (view.sessions[0].round_passes, view.sessions[0].total_passes),
        (1, 1)
    );
    assert_eq!(
        view.sessions[0]
            .owner
            .release
            .release
            .as_ref()
            .unwrap()
            .retry_attempts,
        0
    );
    assert!(!c.try_join().unwrap());
    for _ in 0..20 {
        assert_eq!(c.request_shutdown().round, 1);
        assert!(!c.shutdown_snapshot().normal_exit_allowed);
    }
    assert_eq!(c.request_release_round().unwrap().round, 1);
    let (p, proof) = payload();
    let denied = c.try_cleanup(p, |_, p| p).unwrap_err();
    assert_eq!(denied.category, WorkerCategory::Full);
    proof.check(&denied.input);
    let (g4, held4) = gate();
    owner.lock().unwrap().release_gate = Some((4, g4));
    held3.release();
    held4.entered();
    check_calls(&owner, &c, 4);
    assert_eq!(c.shutdown_snapshot().sessions[0].round_passes, 2);
    assert_eq!(
        c.shutdown_snapshot().sessions[0]
            .owner
            .release
            .release
            .as_ref()
            .unwrap()
            .retry_attempts,
        1
    );
    held4.release();
    let ended = report(&c);
    let r = &ended.sessions[0];
    assert_eq!(r.registration, reg.registration);
    assert_eq!(r.retries.len(), 2);
    let end_error = r.end.as_ref().unwrap().original.as_ref().unwrap_err();
    assert_eq!(release_error(end_error).errors[0].category(), first);
    let retry_error = r.retries[0].original.as_ref().unwrap_err();
    assert_eq!(release_error(retry_error).errors[0].category(), latest);
    assert!(r.retries[1].original.is_ok());
    let prior = r.retries[1].before.release.as_ref().unwrap();
    same_error(&prior.first.errors[0], &release_error(end_error).errors[0]);
    same_error(
        &prior.latest.errors[0],
        &release_error(retry_error).errors[0],
    );
    assert_eq!(prior.retry_attempts, 1);
    assert!(r.retries[1].after.release.is_none());
    let view = c.shutdown_snapshot();
    assert_eq!(
        (view.sessions[0].round_passes, view.sessions[0].total_passes),
        (2, 2)
    );
    assert!(view.sessions[0].first_failure.is_some());
    no_secrets(&format!("{view:?} {ended:?}"), &f.root);
    {
        let calls = &owner.lock().unwrap().calls;
        assert!(calls[0].0 == calls[2].0 && calls[2].0 == calls[3].0 && calls[1].0 != calls[0].0);
    }
    drop(denied);
    complete(&mut c, true);
    check_calls(&owner, &c, 4);
}

#[test]
fn s4_zero_one_two_bounds_fair_passes_duplicate_and_explicit_new_round() {
    for (policy, budget) in [
        (RetryPolicy::None, 0),
        (RetryPolicy::Once, 1),
        (RetryPolicy::Twice, 2),
    ] {
        let (_f, mut c, client) = start::<Payload, Receipt>(2, 2);
        let owner = release_owner(vec![
            Some(LockErrorCategory::LockReleaseFailed);
            2 * (1 + budget)
        ]);
        let a = result(
            &c,
            &client
                .try_submit(
                    (owner.clone(), ProbeOwner::default(), targets()),
                    begin_release,
                )
                .unwrap(),
        );
        let b = result(
            &c,
            &client
                .try_submit(
                    (owner.clone(), ProbeOwner::default(), targets()),
                    begin_release,
                )
                .unwrap(),
        );
        c.request_shutdown_with_policy(policy);
        let ended = report(&c);
        settled(&c);
        assert_eq!(ended.sessions.len(), 2);
        let initial = c.shutdown_snapshot();
        assert!(initial.blockers.contains(&ShutdownBlocker::ReleaseBudget));
        assert_eq!(initial.round, 1);
        for session in &initial.sessions {
            assert!(session.owner.release_pending);
            assert_eq!(usize::from(session.round_passes), budget);
            assert_eq!(session.total_passes, budget as u64);
        }
        let a_key = a.key.unwrap();
        let b_key = b.key.unwrap();
        {
            let s = owner.lock().unwrap();
            assert_eq!(s.calls.len(), 2 * (1 + budget));
            for pair in s.calls.as_chunks::<2>().0 {
                assert!(pair[0].1 == a_key.session && pair[1].1 == b_key.session);
            }
            assert!(s
                .calls
                .iter()
                .all(|call| Some(call.3) == c.snapshot().thread));
        }
        drop(client);
        for _ in 0..30 {
            assert_eq!(c.request_shutdown().round, 1);
            assert_eq!(
                c.shutdown_snapshot().sessions[0].total_passes,
                budget as u64
            );
        }
        settled(&c);
        assert_eq!(owner.lock().unwrap().calls.len(), 2 * (1 + budget));
        owner.lock().unwrap().responses.extend([None, None]);
        assert_eq!(c.request_release_round().unwrap().round, 2);
        let resumed = report(&c);
        assert_eq!(resumed.round, 2);
        assert!(resumed
            .sessions
            .iter()
            .all(|s| s.end.is_none() && s.retries.len() == 1 && s.retries[0].original.is_ok()));
        let view = c.shutdown_snapshot();
        for (old, new) in initial.sessions.iter().zip(&view.sessions) {
            assert_eq!((new.round_passes, new.total_passes), (1, budget as u64 + 1));
            same_error(
                &old.first_failure
                    .as_ref()
                    .unwrap()
                    .release
                    .as_ref()
                    .unwrap()
                    .first
                    .errors[0],
                &new.first_failure
                    .as_ref()
                    .unwrap()
                    .release
                    .as_ref()
                    .unwrap()
                    .first
                    .errors[0],
            );
        }
        complete(&mut c, true);
        assert_eq!(owner.lock().unwrap().calls.len(), 2 * (2 + budget));
        assert_eq!(owner.lock().unwrap().acquire_count, 2);
        assert_eq!(
            c.request_release_round().unwrap_err(),
            ShutdownRequestError::NoReleaseWork
        );
    }
}

#[test]
fn s5_existing_release_failure_partial_acquire_same_owner_and_invalid_registration() {
    for partial in [false, true] {
        let (_f, mut c, client) = start::<Payload, Receipt>(2, 2);
        let (_other_f, mut other, other_client) = start::<Payload, Receipt>(1, 1);
        let foreign = result(
            &other,
            &other_client
                .try_submit(ProbeOwner::default(), register)
                .unwrap(),
        );
        let owner = release_owner(vec![
            Some(LockErrorCategory::LockReleaseFailed),
            Some(LockErrorCategory::LockLost),
            None,
        ]);
        let mut paths = targets();
        if partial {
            owner.lock().unwrap().fail_acquire = Some(2);
            paths.push(ProjectRelativePath::parse("templates/other.json").unwrap());
        }
        let reg = result(
            &c,
            &client
                .try_submit((owner.clone(), ProbeOwner::default(), paths), begin_release)
                .unwrap(),
        );
        assert_eq!(reg.original.is_err(), partial);
        c.close_admission();
        if !partial {
            let (error, observed) = end_failed(&c, &reg.registration);
            assert!(observed.release.is_some());
            assert!(matches!(error, EditSessionError::Release { .. }));
        }
        let acquired = owner.lock().unwrap().acquire_count;
        let stale = result(
            &c,
            &c.try_cleanup(reg.registration.clone(), |ctx, reg| {
                let invalid = Registration {
                    worker: reg.worker.clone(),
                    serial: reg.serial + 100,
                };
                assert_eq!(
                    ctx.retry_session_release(&invalid).unwrap_err(),
                    WorkerCategory::Stale
                );
                invalid
            })
            .unwrap(),
        );
        for invalid in [stale, foreign.registration.clone()] {
            result(
                &c,
                &c.try_cleanup(invalid, |ctx, invalid| {
                    assert_eq!(
                        ctx.retry_session_release(&invalid).unwrap_err(),
                        WorkerCategory::Stale
                    )
                })
                .unwrap(),
            );
        }
        assert_eq!(owner.lock().unwrap().calls.len(), 1);
        c.request_shutdown();
        wait_for(&c, |m| m.shutdown.has_report() && !m.running);
        result(
            &c,
            &c.try_cleanup(reg.registration.clone(), |ctx, reg| {
                assert_eq!(
                    ctx.retry_session_release(&reg).unwrap_err(),
                    WorkerCategory::Stale
                );
            })
            .unwrap(),
        );
        let r = report(&c);
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].registration, reg.registration);
        assert!(r.sessions[0].end.is_none());
        assert_eq!(r.sessions[0].retries.len(), 2);
        assert!(
            r.sessions[0].retries[0].original.is_err() && r.sessions[0].retries[1].original.is_ok()
        );
        let first = c.shutdown_snapshot().sessions[0]
            .first_failure
            .clone()
            .unwrap();
        assert_eq!(first.acquire_error.is_some(), partial);
        if let Some(error) = first.acquire_error {
            assert_eq!(error.category(), LockErrorCategory::LockAcquireFailed);
        }
        assert_eq!(owner.lock().unwrap().acquire_count, acquired);
        check_calls(&owner, &c, 3);
        complete(&mut c, true);
        other.request_shutdown();
        complete(&mut other, true);
    }
}

#[test]
fn s6_s7_s8_two_payloads_real_receipts_saturated_results_and_same_shutdown_after_claim() {
    for blocked in [false, true] {
        let (f, mut c, client) = start::<Payload, Receipt>(2, 2);
        let (p1, proof1) = payload();
        let (p2, proof2) = payload();
        assert!(!Arc::ptr_eq(&proof1.owner, &proof2.owner));
        let sink1 = SinkOwner::default();
        let sink2 = SinkOwner::default();
        sink2.lock().unwrap().fail_next = true;
        let provider = ProbeOwner::default();
        let obstacle = blocked.then(|| f.root.join(".worldbuild"));
        let (r1, r2) = result(
            &c,
            &client
                .try_submit(
                    (p1, p2, sink1.clone(), provider.clone(), obstacle.clone()),
                    |ctx, (p1, p2, sink, provider, obstacle)| {
                        let r1 = register(ctx, provider.clone());
                        let r2 = register(ctx, provider);
                        ctx.session(r1.key.as_ref().unwrap())
                            .unwrap()
                            .bind_draft(p1)
                            .unwrap();
                        ctx.session(r2.key.as_ref().unwrap())
                            .unwrap()
                            .bind_draft(p2)
                            .unwrap();
                        connect(ctx, (r1.key.as_ref().unwrap().clone(), sink));
                        ctx.runtime.as_mut().unwrap().invalidate_recovery();
                        if let Some(obstacle) = obstacle {
                            assert!(!obstacle.exists());
                            fs::write(obstacle, b"G12 blocked handoff evidence").unwrap();
                            assert!(ctx.recover().is_err());
                            assert_eq!(
                                ctx.runtime_snapshot().unwrap().state,
                                RuntimeState::Blocked
                            );
                        }
                        for r in [&r1, &r2] {
                            assert_eq!(
                                ctx.session(r.key.as_ref().unwrap())
                                    .unwrap()
                                    .preserve_existing()
                                    .unwrap(),
                                CustodyTransition::Preserved
                            );
                        }
                        ctx.session(r1.key.as_ref().unwrap())
                            .unwrap()
                            .accept()
                            .unwrap();
                        (r1, r2)
                    },
                )
                .unwrap(),
        );
        proof1.check(sink1.lock().unwrap().accepted.as_ref().unwrap());
        // 완료된 일반 결과 두 개와 실제 R 제어 결과를 동시에 남긴다.
        let a = client.try_submit(19_u32, |_, n| n).unwrap();
        let b = client.try_submit(23_u32, |_, n| n).unwrap();
        wait_for(&c, |m| {
            m.results.values().all(|v| matches!(v, Stored::Complete(_)))
        });
        let aid = a.id();
        let bid = b.id();
        drop((a, b));
        c.close_admission();
        let ack1 = c
            .try_cleanup(r1.registration.clone(), |ctx, r| {
                ctx.acknowledge(&r).unwrap().unwrap()
            })
            .unwrap();
        let ack1id = ack1.id();
        drop(ack1);
        wait_for(&c, |m| {
            matches!(m.control_result, Some((_, Stored::Complete(_))))
        });
        c.request_shutdown();
        wait_for(&c, |m| {
            m.shutdown.view.phase == ShutdownPhase::WaitingControl
        });
        drop(client);
        assert_eq!(count(&provider, "release"), 0);
        assert_eq!(c.snapshot().ordinary_outstanding, 2);
        assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
        for _ in 0..10 {
            assert_eq!(c.request_shutdown().round, 1);
        }
        assert_eq!(
            c.try_take::<u32>(&ack1id).unwrap_err(),
            WorkerCategory::ResultType
        );
        let original_id = c.pending_cleanup_id().unwrap();
        assert!(
            original_id.serial == ack1id.serial
                && original_id.control
                && Arc::ptr_eq(&original_id.worker, &ack1id.worker)
        );
        let (p3, proof3) = payload();
        let duplicate = c.try_cleanup(p3, |_, p| p).unwrap_err();
        assert_eq!(duplicate.category, WorkerCategory::Full);
        proof3.check(&duplicate.input);
        let ack1: Acknowledged<Receipt> = c.try_take(&original_id).unwrap().unwrap();
        check_receipt(&ack1, &sink1);
        assert_eq!(
            ack1.before.handoff,
            Some(RecoveryHandoffStatus::DurablyAccepted)
        );
        assert_eq!(
            ack1.after.handoff,
            Some(RecoveryHandoffStatus::Acknowledged)
        );
        assert_eq!(ack1.after.session, EditSessionState::RecoveryRequired);
        assert!(!ack1.after.normal_exit_allowed);
        assert_eq!(
            c.try_take::<u32>(&ack1id).unwrap_err(),
            WorkerCategory::UnknownRequest
        );
        let ended = report(&c);
        assert_eq!(ended.sessions.len(), 2);
        assert_eq!(count(&provider, "release"), 2);
        settled(&c);
        let view = c.shutdown_snapshot();
        assert!(view.blockers.contains(&ShutdownBlocker::PendingHandoff));
        assert!(view.blockers.contains(&ShutdownBlocker::RuntimeRecovery));
        assert!(view.blockers.contains(&ShutdownBlocker::Results));
        assert!(view
            .sessions
            .iter()
            .any(|s| s.owner.custody == Custody::Pending && !s.owner.release_pending));
        assert_eq!(sink2.lock().unwrap().calls, 0);
        // 미연결 경로는 그대로 차단하며, 명시적으로 만든 실제 sink만 연결한다.
        result(
            &c,
            &c.try_cleanup(
                (r2.registration.clone(), sink2.clone()),
                |ctx, (r, sink)| {
                    assert!(ctx.accept(&r).unwrap().is_err());
                    assert_eq!(
                        ctx.remove_session(&r).unwrap_err(),
                        WorkerCategory::OwnersRemain
                    );
                    assert_eq!(
                        ctx.close_runtime().unwrap_err(),
                        WorkerCategory::OwnersRemain
                    );
                    let backend = RecoveryBackend::connected(Sink {
                        owner: sink,
                        local_calls: Rc::new(Cell::new(0)),
                    });
                    assert!(ctx.connect_backend(&r, backend).is_ok());
                    let failed = ctx.accept(&r).unwrap().unwrap_err();
                    let observed = ctx.session_release_observation(&r).unwrap();
                    assert_eq!(
                        observed.snapshot.recovery_handoff(),
                        Some(RecoveryHandoffStatus::Pending)
                    );
                    failed
                },
            )
            .unwrap(),
        );
        assert_eq!(proof2.drops.load(Ordering::SeqCst), 0);
        assert_eq!(sink2.lock().unwrap().seen_address, Some(proof2.address));
        result(
            &c,
            &c.try_cleanup(r2.registration.clone(), |ctx, r| {
                ctx.accept(&r).unwrap().unwrap()
            })
            .unwrap(),
        );
        proof2.check(sink2.lock().unwrap().accepted.as_ref().unwrap());
        settled(&c);
        assert!(c
            .shutdown_snapshot()
            .blockers
            .contains(&ShutdownBlocker::Receipt));
        let ack2 = c
            .try_cleanup(r2.registration.clone(), |ctx, r| {
                let before = ctx.session_release_observation(&r).unwrap().snapshot;
                let ack = ctx.acknowledge(&r).unwrap().unwrap();
                let after = ctx.session_release_observation(&r).unwrap().snapshot;
                (ack, before, after)
            })
            .unwrap();
        let ack2id = ack2.id();
        drop(ack2);
        wait_for(&c, |m| {
            matches!(m.control_result, Some((_, Stored::Complete(_))))
        });
        assert_eq!(
            c.try_take::<()>(&ack2id).unwrap_err(),
            WorkerCategory::ResultType
        );
        let (ack2, before, after): (
            Acknowledged<Receipt>,
            crate::data::edit_session::EditSessionSnapshot,
            crate::data::edit_session::EditSessionSnapshot,
        ) = c.try_take(&ack2id).unwrap().unwrap();
        check_receipt(&ack2, &sink2);
        assert_eq!(
            ack2.before.handoff,
            Some(RecoveryHandoffStatus::DurablyAccepted)
        );
        assert_eq!(
            ack2.after.handoff,
            Some(RecoveryHandoffStatus::Acknowledged)
        );
        assert_eq!(ack2.after.session, EditSessionState::ReadOnly);
        assert!(before.session_id().is_some() && after.session_id().is_none());
        assert!(!ack2.after.normal_exit_allowed);
        assert_eq!(
            c.snapshot().runtime.unwrap().state,
            if blocked {
                RuntimeState::Blocked
            } else {
                RuntimeState::Pending
            }
        );
        assert_eq!(
            c.try_take::<()>(&ack2id).unwrap_err(),
            WorkerCategory::UnknownRequest
        );
        assert_eq!(
            c.try_take::<()>(&aid).unwrap_err(),
            WorkerCategory::ResultType
        );
        assert_eq!(c.try_take::<u32>(&aid).unwrap(), Some(19));
        assert_eq!(c.try_take::<u32>(&bid).unwrap(), Some(23));
        assert_eq!(
            c.try_take::<u32>(&aid).unwrap_err(),
            WorkerCategory::UnknownRequest
        );
        settled(&c);
        assert!(c
            .shutdown_snapshot()
            .blockers
            .contains(&ShutdownBlocker::RuntimeRecovery));
        if let Some(obstacle) = obstacle {
            assert!(fs::read(&obstacle).unwrap() == b"G12 blocked handoff evidence");
            fs::remove_file(obstacle).unwrap();
        }
        let recovered = result(
            &c,
            &c.try_cleanup((), |ctx, ()| ctx.recover_runtime()).unwrap(),
        )
        .unwrap();
        assert!(!recovered.manual_recovery_required);
        let reports = complete(&mut c, true);
        assert!(reports
            .iter()
            .any(|r| matches!(r.close, Some(Ok(s)) if s.state == RuntimeState::Ready)));
        assert!(c.snapshot().admission_closed);
        assert_eq!(count(&provider, "release"), 2);
        no_secrets(
            &format!("{:?} {:?} {:?}", c.shutdown_snapshot(), ended, reports),
            &f.root,
        );
        drop((ack1, ack2, duplicate));
        assert_eq!(
            sink1.lock().unwrap().receipt_drops.load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            sink2.lock().unwrap().receipt_drops.load(Ordering::SeqCst),
            1
        );
        assert_eq!(proof3.drops.load(Ordering::SeqCst), 1);
        drop((sink1, sink2));
        assert_eq!(proof1.drops.load(Ordering::SeqCst), 1);
        assert_eq!(proof2.drops.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn s6_s8_active_return_caller_owner_survives_registration_removal_and_resource_join() {
    let (_f, mut c, client) = start::<Payload, Receipt>(1, 1);
    let (p, proof) = payload();
    let owner = ProbeOwner::default();
    let reg = result(
        &c,
        &client
            .try_submit((p, owner.clone()), |ctx, (p, owner)| {
                let reg = register(ctx, owner);
                ctx.session(reg.key.as_ref().unwrap())
                    .unwrap()
                    .bind_draft(p)
                    .unwrap();
                reg
            })
            .unwrap(),
    );
    c.request_shutdown();
    let ended = report(&c);
    assert!(ended.sessions[0].end.as_ref().unwrap().original.is_ok());
    settled(&c);
    assert!(c
        .shutdown_snapshot()
        .blockers
        .contains(&ShutdownBlocker::ActiveDraft));
    assert_eq!(proof.drops.load(Ordering::SeqCst), 0);
    let t = c
        .try_cleanup(reg.registration, |ctx, r| {
            assert_eq!(
                ctx.remove_session(&r).unwrap_err(),
                WorkerCategory::OwnersRemain
            );
            assert_eq!(
                ctx.close_runtime().unwrap_err(),
                WorkerCategory::OwnersRemain
            );
            let p = ctx.return_active(&r).unwrap();
            ctx.remove_session(&r).unwrap();
            p
        })
        .unwrap();
    let id = t.id();
    drop((t, client));
    wait_for(&c, |m| {
        matches!(m.control_result, Some((_, Stored::Complete(_))))
    });
    assert_eq!(
        c.try_take::<()>(&id).unwrap_err(),
        WorkerCategory::ResultType
    );
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    let original: Payload = c.try_take(&id).unwrap().unwrap();
    proof.check(&original);
    complete(&mut c, false);
    let view = c.shutdown_snapshot();
    assert_eq!(view.caller_custody, 1);
    assert!(view.blockers.contains(&ShutdownBlocker::CallerCustody));
    assert_eq!(count(&owner, "release"), 1);
    proof.check(&original);
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
    assert!(!c.request_shutdown().normal_exit_allowed);
}

#[test]
fn s6_s9_pending_recovery_failure_then_actual_success_does_not_reset_release_budget() {
    let (f, mut c, client) = start::<Payload, Receipt>(2, 1);
    let owner = release_owner(vec![Some(LockErrorCategory::LockReleaseFailed); 3]);
    let reg = result(
        &c,
        &client
            .try_submit(
                (owner.clone(), ProbeOwner::default(), targets()),
                begin_release,
            )
            .unwrap(),
    );
    result(
        &c,
        &client
            .try_submit((), |ctx, ()| {
                ctx.runtime.as_mut().unwrap().invalidate_recovery()
            })
            .unwrap(),
    );
    let evidence = f.root.join(".worldbuild");
    // transaction 없는 fixture의 부재를 확인한 뒤 실제 recovery 진입을 막는 파일을 만든다.
    assert!(!evidence.exists());
    fs::write(&evidence, b"G12 recovery evidence").unwrap();
    c.request_shutdown();
    let ended = report(&c);
    assert_eq!(ended.sessions[0].registration, reg.registration);
    assert_eq!(ended.sessions[0].retries.len(), 2);
    let failed = result(
        &c,
        &c.try_cleanup((), |ctx, ()| ctx.recover_runtime()).unwrap(),
    )
    .unwrap_err();
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Blocked);
    no_secrets(&format!("{failed:?} {failed}"), &f.root);
    assert!(fs::read(&evidence).unwrap() == b"G12 recovery evidence");
    fs::remove_file(&evidence).unwrap();
    result(
        &c,
        &c.try_cleanup((), |ctx, ()| ctx.recover_runtime().unwrap())
            .unwrap(),
    );
    settled(&c);
    assert_eq!(c.snapshot().runtime.unwrap().state, RuntimeState::Ready);
    assert_eq!(c.shutdown_snapshot().sessions[0].total_passes, 2);
    assert_eq!(owner.lock().unwrap().calls.len(), 3);
    assert!(c
        .shutdown_snapshot()
        .blockers
        .contains(&ShutdownBlocker::ReleaseBudget));
    owner.lock().unwrap().responses.push_back(None);
    c.request_release_round().unwrap();
    let resumed = report(&c);
    assert!(resumed.sessions[0].retries[0].original.is_ok());
    complete(&mut c, true);
    check_calls(&owner, &c, 4);
}

#[test]
fn s9_initial_acquire_failure_and_retained_runtime_recovery_failure_are_distinct() {
    let f = fixture::Fixture::new();
    let mut cfg = config(&f, 1, 1);
    cfg.project_root = f.root.join("missing");
    let (mut c, client) = WorkerControl::<(), ()>::start(cfg).unwrap();
    wait_for(&c, |m| m.status == WorkerStatus::InitializationFailed);
    c.request_shutdown();
    assert!(c.snapshot().runtime.is_none());
    join(&mut c);
    assert!(!c.shutdown_snapshot().resources_complete);
    let first = c.take_shutdown_report().unwrap();
    assert!(first.initialization_error.is_some() && first.close.is_none());
    assert!(c.shutdown_snapshot().resources_complete && !c.shutdown_snapshot().normal_exit_allowed);
    assert_eq!(c.request_shutdown().phase, ShutdownPhase::Joined);
    assert!(c.take_shutdown_report().is_none());
    drop(client);
    let f2 = fixture::Fixture::new();
    fs::write(
        f2.root.join(".worldbuild"),
        b"G12 initial recovery evidence",
    )
    .unwrap();
    let (mut c2, client2) = WorkerControl::<(), ()>::start(config(&f2, 1, 1)).unwrap();
    wait_for(&c2, |m| m.status == WorkerStatus::InitializationFailed);
    c2.request_shutdown();
    let initial = report(&c2);
    assert!(initial.initialization_error.is_some() && initial.close.is_none());
    settled(&c2);
    assert_eq!(
        c2.shutdown_snapshot().runtime.unwrap().state,
        RuntimeState::Blocked
    );
    assert!(!c2.try_join().unwrap());
    assert!(result(
        &c2,
        &c2.try_cleanup((), |ctx, ()| ctx.recover_runtime()).unwrap()
    )
    .is_err());
    assert!(fs::read(f2.root.join(".worldbuild")).unwrap() == b"G12 initial recovery evidence");
    fs::remove_file(f2.root.join(".worldbuild")).unwrap();
    result(
        &c2,
        &c2.try_cleanup((), |ctx, ()| ctx.recover_runtime().unwrap())
            .unwrap(),
    );
    complete(&mut c2, true);
    drop(client2);
}

#[test]
fn s9_consumptive_runtime_close_failure_keeps_original_once_and_no_reacquire() {
    let (f, mut c, client) = start::<(), ()>(1, 1);
    result(
        &c,
        &client
            .try_submit((), |ctx, ()| {
                ctx.close_runtime_with =
                    |runtime| crate::data::project_runtime::with_release_fault(|| runtime.close());
            })
            .unwrap(),
    );
    c.request_shutdown();
    wait_for(&c, |m| m.shutdown.has_report() && !m.running);
    assert!(c.snapshot().runtime.is_none());
    let retry = c
        .try_cleanup((), |ctx, ()| ctx.close_runtime().unwrap_err())
        .unwrap();
    assert_eq!(result(&c, &retry), WorkerCategory::Unavailable);
    for _ in 0..10 {
        assert_eq!(c.request_shutdown().round, 1);
    }
    let reports = complete(&mut c, false);
    assert_eq!(reports.len(), 1);
    let error = reports[0].close.as_ref().unwrap().as_ref().unwrap_err();
    assert_eq!(error.snapshot.state, RuntimeState::Ready);
    assert!(error.previous_failure.is_none());
    let view = c.shutdown_snapshot();
    assert!(view.blockers.contains(&ShutdownBlocker::RuntimeCloseFailed));
    assert!(matches!(view.close, Some(CloseObservation::Failed { .. })));
    assert_eq!(
        c.request_release_round().unwrap_err(),
        ShutdownRequestError::NoReleaseWork
    );
    no_secrets(&format!("{view:?} {error:?} {error}"), &f.root);
    let mut source = std::error::Error::source(error.as_ref());
    while let Some(error) = source {
        no_secrets(&format!("{error:?} {error}"), &f.root);
        source = error.source();
    }
    // 성공과 실패 양쪽 모두 실제 guard가 소비되어 명시적 외부 acquire가 가능하다.
    ProjectRuntime::acquire(&f.root, &f.root.parent().unwrap().join("locks"))
        .unwrap()
        .close()
        .unwrap();
}

#[test]
fn s9_unavailable_keeps_original_result_and_never_issues_normal_completion() {
    let (_f, mut c, client) = start::<(), ()>(2, 1);
    let (p, proof) = payload();
    let t = client.try_submit(p, |_, p| p).unwrap();
    let id = t.id();
    drop(t);
    wait_for(&c, |m| {
        matches!(m.results.get(&id.serial), Some(Stored::Complete(_)))
    });
    c.close_admission();
    result(
        &c,
        &c.try_cleanup((), |ctx, ()| ctx.close_runtime().unwrap().unwrap())
            .unwrap(),
    );
    // 실제 자원을 닫은 뒤에만 panic 경계를 검사한다. 임의 panic의 데이터 보존은 보장하지 않는다.
    let panic = c
        .try_cleanup((), |_, ()| -> () {
            panic!("G12 controlled worker failure")
        })
        .unwrap();
    wait_for(&c, |m| m.status == WorkerStatus::Unavailable);
    drop((panic, client));
    c.request_shutdown();
    join(&mut c);
    for _ in 0..10 {
        let view = c.request_shutdown();
        assert_eq!(view.phase, ShutdownPhase::Unavailable);
        assert!(!view.resources_complete && !view.normal_exit_allowed);
    }
    assert_eq!(
        c.try_take::<()>(&id).unwrap_err(),
        WorkerCategory::ResultType
    );
    let original: Payload = c.try_take(&id).unwrap().unwrap();
    proof.check(&original);
    assert_eq!(
        c.try_take::<()>(&id).unwrap_err(),
        WorkerCategory::UnknownRequest
    );
    drop(original);
    assert_eq!(proof.drops.load(Ordering::SeqCst), 1);
}

#[test]
fn s10_worker_local_non_send_opaque_payload_receipt_and_sink_keep_actual_identity() {
    struct LocalValue {
        bytes: Vec<u8>,
        owner: Rc<Cell<usize>>,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for LocalValue {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    struct LocalSink {
        accepted: Option<LocalValue>,
        proof: Arc<Mutex<Vec<usize>>>,
        drops: Arc<AtomicUsize>,
    }
    impl DurableRecoverySink<LocalValue> for LocalSink {
        type Receipt = LocalValue;
        fn accept_durably(
            &mut self,
            _: &RecoveryEnvelope,
            p: LocalValue,
        ) -> Result<LocalValue, RecoverySinkFailure<LocalValue>> {
            assert!(p.bytes == SECRET.as_bytes());
            let proof = self.proof.lock().unwrap();
            assert!(
                p.bytes.as_ptr() as usize == proof[0] && Rc::as_ptr(&p.owner) as usize == proof[1]
            );
            assert_eq!(p.owner.get(), 17);
            drop(proof);
            self.accepted = Some(p);
            let r = LocalValue {
                bytes: RECEIPT.as_bytes().to_vec(),
                owner: Rc::new(Cell::new(31)),
                drops: self.drops.clone(),
            };
            self.proof
                .lock()
                .unwrap()
                .extend([r.bytes.as_ptr() as usize, Rc::as_ptr(&r.owner) as usize]);
            Ok(r)
        }
    }
    let (f, mut c, client) = start::<LocalValue, LocalValue>(2, 1);
    let proof = Arc::new(Mutex::new(Vec::new()));
    let pdrops = Arc::new(AtomicUsize::new(0));
    let rdrops = Arc::new(AtomicUsize::new(0));
    let reg = result(
        &c,
        &client
            .try_submit(
                (proof.clone(), pdrops.clone(), rdrops.clone()),
                |ctx, (proof, pdrops, rdrops)| {
                    let reg = register(ctx, ProbeOwner::default());
                    let p = LocalValue {
                        bytes: SECRET.as_bytes().to_vec(),
                        owner: Rc::new(Cell::new(17)),
                        drops: pdrops,
                    };
                    proof
                        .lock()
                        .unwrap()
                        .extend([p.bytes.as_ptr() as usize, Rc::as_ptr(&p.owner) as usize]);
                    ctx.session(reg.key.as_ref().unwrap())
                        .unwrap()
                        .bind_draft(p)
                        .unwrap();
                    assert!(ctx
                        .session(reg.key.as_ref().unwrap())
                        .unwrap()
                        .connect_backend(RecoveryBackend::connected(LocalSink {
                            accepted: None,
                            proof,
                            drops: rdrops
                        }))
                        .is_ok());
                    ctx.runtime.as_mut().unwrap().invalidate_recovery();
                    ctx.session(reg.key.as_ref().unwrap())
                        .unwrap()
                        .preserve_existing()
                        .unwrap();
                    ctx.session(reg.key.as_ref().unwrap())
                        .unwrap()
                        .accept()
                        .unwrap();
                    reg
                },
            )
            .unwrap(),
    );
    c.request_shutdown();
    let ended = report(&c);
    settled(&c);
    assert!(c
        .shutdown_snapshot()
        .blockers
        .contains(&ShutdownBlocker::Receipt));
    result(
        &c,
        &c.try_cleanup((reg.registration, proof), |ctx, (reg, proof)| {
            let ack = ctx.acknowledge(&reg).unwrap().unwrap();
            assert!(ack.receipt.bytes == RECEIPT.as_bytes());
            let proof = proof.lock().unwrap();
            assert!(
                ack.receipt.bytes.as_ptr() as usize == proof[2]
                    && Rc::as_ptr(&ack.receipt.owner) as usize == proof[3]
            );
            assert_eq!(ack.receipt.owner.get(), 31);
            assert_eq!(Rc::strong_count(&ack.receipt.owner), 1);
            assert!(ctx
                .session_release_observation(&reg)
                .unwrap()
                .snapshot
                .session_id()
                .is_none());
            assert!(!ack.after.normal_exit_allowed);
            drop(ack);
            ctx.recover_runtime().unwrap();
        })
        .unwrap(),
    );
    complete(&mut c, true);
    assert_eq!(pdrops.load(Ordering::SeqCst), 1);
    assert_eq!(rdrops.load(Ordering::SeqCst), 1);
    let mut text = format!("{:?} {ended:?}", c.shutdown_snapshot());
    for error in [
        ShutdownRequestError::NotStarted,
        ShutdownRequestError::ResultsRemain,
        ShutdownRequestError::NoReleaseWork,
        ShutdownRequestError::Exhausted,
        ShutdownRequestError::Unavailable,
    ] {
        text.push_str(&format!("{error:?} {error}"));
        assert!(std::error::Error::source(&error).is_none());
    }
    for blocker in [
        ShutdownBlocker::Results,
        ShutdownBlocker::ReleaseBudget,
        ShutdownBlocker::ActiveDraft,
        ShutdownBlocker::PendingHandoff,
        ShutdownBlocker::Receipt,
        ShutdownBlocker::ValidationFailure,
        ShutdownBlocker::RuntimeRecovery,
        ShutdownBlocker::ClosedRecovery,
        ShutdownBlocker::RuntimeCloseFailed,
        ShutdownBlocker::CallerCustody,
        ShutdownBlocker::InitializationFailed,
        ShutdownBlocker::Unavailable,
    ] {
        text.push_str(&format!("{blocker:?} {blocker}"));
    }
    no_secrets(&text, &f.root);
}

#[test]
fn s5_s8_explicit_cleanup_removed_owner_keeps_history_without_phantom_retry() {
    let (_f, mut c, client) = start::<Payload, Receipt>(1, 1);
    let owner = release_owner(vec![Some(LockErrorCategory::LockReleaseFailed), None]);
    let reg = result(
        &c,
        &client
            .try_submit(
                (owner.clone(), ProbeOwner::default(), targets()),
                begin_release,
            )
            .unwrap(),
    );
    c.request_shutdown_with_policy(RetryPolicy::None);
    wait_for(&c, |m| m.shutdown.has_report() && !m.running);
    let cleanup = result(
        &c,
        &c.try_cleanup(reg.registration.clone(), |ctx, r| {
            let retry = ctx.retry_session_release(&r).unwrap();
            assert!(retry.original.is_ok());
            ctx.remove_session(&r).unwrap();
            assert_eq!(
                ctx.retry_session_release(&r).unwrap_err(),
                WorkerCategory::Stale
            );
            retry
        })
        .unwrap(),
    );
    let initial = report(&c);
    assert!(initial.sessions[0].end.as_ref().unwrap().original.is_err());
    same_error(
        &cleanup.before.release.unwrap().first.errors[0],
        &release_error(
            initial.sessions[0]
                .end
                .as_ref()
                .unwrap()
                .original
                .as_ref()
                .unwrap_err(),
        )
        .errors[0],
    );
    complete(&mut c, true);
    let view = c.shutdown_snapshot();
    assert!(!view.sessions[0].registered && !view.sessions[0].owner.release_pending);
    assert_eq!(view.sessions[0].total_passes, 0);
    assert!(view.sessions[0].first_failure.is_some());
    check_calls(&owner, &c, 2);
}

#[test]
fn s8_unclaimed_validation_failure_blocks_removal_close_and_stop_until_original_claim() {
    let (_f, mut c, client) = start::<(), ()>(1, 1);
    let owner = ProbeOwner::default();
    let reg = result(&c, &client.try_submit(owner.clone(), register).unwrap());
    let key = reg.key.unwrap();
    owner.lock().unwrap().fail = true;
    client.trigger(&key, TriggerReason::Foreground).unwrap();
    wait_validations(&c, &key, 1);
    c.request_shutdown();
    wait_for(&c, |m| m.shutdown.has_report() && !m.running);
    assert_eq!(c.snapshot().unclaimed_validation_failures, 1);
    assert_eq!(c.request_stop().unwrap_err(), WorkerCategory::OwnersRemain);
    result(
        &c,
        &c.try_cleanup(reg.registration, |ctx, r| {
            assert_eq!(
                ctx.remove_session(&r).unwrap_err(),
                WorkerCategory::OwnersRemain
            );
            assert_eq!(
                ctx.close_runtime().unwrap_err(),
                WorkerCategory::OwnersRemain
            );
        })
        .unwrap(),
    );
    let original = c.take_validation_failure(&key).unwrap();
    assert!(original.preserve.is_none());
    assert_eq!(
        original.original.operation(),
        EditSessionOperation::Revalidate
    );
    assert!(original.reasons.contains(TriggerReason::Foreground));
    assert!(c.take_validation_failure(&key).is_none());
    let reports = complete(&mut c, true);
    assert!(reports.iter().any(|r| r
        .coordination_errors
        .contains(&WorkerCategory::OwnersRemain)));
    assert_eq!(count(&owner, "acquire"), 1);
    assert_eq!(count(&owner, "release"), 1);
}

#[test]
fn s9_prior_blocked_runtime_close_preserves_previous_cause_and_actual_closed_owner() {
    for fault in [false, true] {
        let f = fixture::Fixture::new();
        fs::write(f.root.join(".worldbuild"), b"G12 retained recovery failure").unwrap();
        let (mut c, client) = WorkerControl::<(), ()>::start(config(&f, 1, 1)).unwrap();
        wait_for(&c, |m| m.status == WorkerStatus::InitializationFailed);
        let previous = c.snapshot().initialization_error.unwrap();
        let closed = result(
            &c,
            &c.try_cleanup(fault, |ctx, fault| {
                if fault {
                    ctx.0.close_runtime_with = |runtime| {
                        crate::data::project_runtime::with_release_fault(|| runtime.close())
                    };
                }
                ctx.close_runtime().unwrap()
            })
            .unwrap(),
        );
        assert!(c.snapshot().runtime.is_none());
        match &closed {
            Ok(snapshot) => {
                assert!(!fault);
                assert_eq!(snapshot.state, RuntimeState::Blocked);
                assert_eq!(snapshot.last_failure, Some(previous.diagnostic()));
            }
            Err(error) => {
                assert!(fault);
                assert_eq!(error.snapshot.state, RuntimeState::Blocked);
                assert_eq!(
                    error.previous_failure.as_ref().unwrap().diagnostic(),
                    previous.diagnostic()
                );
            }
        }
        c.request_shutdown();
        let reports = complete(&mut c, false);
        assert_eq!(reports.len(), 1);
        assert_eq!(
            reports[0]
                .initialization_error
                .as_ref()
                .unwrap()
                .diagnostic(),
            previous.diagnostic()
        );
        assert!(reports[0].close.is_none());
        let view = c.shutdown_snapshot();
        assert!(view.blockers.contains(&if fault {
            ShutdownBlocker::RuntimeCloseFailed
        } else {
            ShutdownBlocker::ClosedRecovery
        }));
        assert!(!view
            .blockers
            .contains(&ShutdownBlocker::InitializationFailed));
        match &view.close {
            Some(CloseObservation::Failed {
                snapshot,
                previous: original,
                release,
            }) => {
                assert_eq!(snapshot.state, RuntimeState::Blocked);
                assert_eq!(
                    original.as_ref().unwrap().diagnostic(),
                    previous.diagnostic()
                );
                assert_eq!(
                    release.diagnostic(),
                    closed.as_ref().unwrap_err().release.diagnostic()
                );
            }
            Some(CloseObservation::Closed(s)) => {
                assert!(!fault);
                assert_eq!(s.state, RuntimeState::Blocked);
            }
            None => panic!("actual consumed runtime observation missing"),
        }
        no_secrets(
            &format!("{view:?} {closed:?} {previous:?} {previous}"),
            &f.root,
        );
        assert!(fs::read(f.root.join(".worldbuild")).unwrap() == b"G12 retained recovery failure");
        drop(client);
    }
}
