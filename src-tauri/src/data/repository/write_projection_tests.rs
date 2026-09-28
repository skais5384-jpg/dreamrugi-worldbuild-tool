use super::*;
use crate::data::{
    atomic_file::SaveOutcome,
    transaction::{
        artifact_diagnostics::{FailureRole, FailureStage, IoContext},
        test_support::{with_commit_io_factory, RecoveryTestPoint},
        CommitFailureSource, CommitOutcome, CommitStage,
    },
};
use std::{cell::RefCell, os::windows::fs::OpenOptionsExt, rc::Rc};

#[derive(Debug)]
struct PrivateCause {
    owner: Arc<u64>,
    nonce: u64,
    secret: &'static str,
}
impl fmt::Display for PrivateCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.secret)
    }
}
impl Error for PrivateCause {}
fn cause(owner: &Arc<u64>, nonce: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        PrivateCause {
            owner: Arc::clone(owner),
            nonce,
            secret: CANARY,
        },
    )
}
fn assert_cause(error: &io::Error, owner: &Arc<u64>, nonce: u64) {
    let original = error
        .get_ref()
        .and_then(|e| e.downcast_ref::<PrivateCause>())
        .expect("retained typed cause");
    assert!(Arc::ptr_eq(&original.owner, owner), "cause owner differs");
    assert_eq!(original.nonce, nonce);
    assert_eq!(*original.owner, 0x8726);
    assert!(original.secret == CANARY, "original canary was replaced");
}

#[test]
fn prepare_primary_typed_owner_and_cleanup_os_error_are_separately_projected() -> TestResult {
    struct Hook {
        owner: Arc<u64>,
        hits: Cell<u32>,
    }
    impl PrepareHooks for Hook {
        fn check(&self, point: PrepareFailPoint, _: Option<u32>) -> io::Result<()> {
            if point == PrepareFailPoint::StagedWrite {
                self.hits.set(self.hits.get() + 1);
                Err(cause(&self.owner, 0x1983))
            } else if point == PrepareFailPoint::Cleanup {
                self.hits.set(self.hits.get() + 1);
                Err(io::Error::from_raw_os_error(5))
            } else {
                Ok(())
            }
        }
    }
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let hook = Hook {
        owner: Arc::new(0x8726),
        hits: Cell::new(0),
    };
    let plan = CanonicalWritePlan::new().create_document(&document())?;
    let error = with_plan(plan, &repo, |plan, permit| {
        prepare_canonical_with_hooks(plan, &repo, permit, &hook).unwrap_err()
    });
    assert_eq!(hook.hits.get(), 2);
    let diagnostic = error.diagnostic();
    assert_eq!(diagnostic.state, CommitResultState::NotApplied);
    assert!(diagnostic.recovery_required);
    assert_eq!(diagnostic.failures.len(), 2);
    let primary = &diagnostic.failures[0];
    assert_eq!(primary.role, FailureRole::Primary);
    assert_eq!(
        primary.stage,
        FailureStage::Prepare(PrepareStage::WriteStaged)
    );
    assert_eq!(
        primary.target,
        Some(ArtifactSourceId::Document(document_id()))
    );
    assert!(primary.transaction_id.is_some());
    assert_eq!(
        primary.io.as_ref().expect("primary IO").kind,
        io::ErrorKind::PermissionDenied
    );
    let cleanup = &diagnostic.failures[1];
    assert_eq!(cleanup.role, FailureRole::Cleanup);
    assert_eq!(cleanup.stage, FailureStage::PrepareCleanup);
    assert_eq!(cleanup.transaction_id, primary.transaction_id);
    assert_eq!(cleanup.io.as_ref().expect("cleanup IO").os_code, Some(5));
    let original = error
        .implementation_original_prepare()
        .expect("retained prepare owner");
    assert_cause(
        original
            .implementation_original_io()
            .expect("retained primary IO"),
        &hook.owner,
        0x1983,
    );
    assert_eq!(
        original
            .cleanup_error()
            .expect("retained cleanup IO")
            .raw_os_error(),
        Some(5)
    );
    no_leak(&error);
    no_leak(diagnostic);
    assert!(!error.to_string().contains(CANARY));
    assert!(error.source().is_none());
    assert!(!fixture
        .path(ArtifactSourceId::Document(document_id()))
        .exists());
    Ok(())
}

fn primary_io(source: &CommitFailureSource) -> &io::Error {
    match source {
        CommitFailureSource::Io(source) => source,
        CommitFailureSource::AtomicSave(crate::data::atomic_file::SaveError::AtomicWrite(
            source,
        )) => &source.source,
        _ => panic!("expected retained IO or atomic IO"),
    }
}
#[test]
fn commit_states_preserve_primary_owner_secondary_stage_and_durability() -> TestResult {
    use CommitTestPoint as C;
    use RecoveryTestPoint as R;
    let cases = [
        (
            C::ManifestRevalidation,
            None,
            None,
            CommitResultState::NotApplied,
            CommitStage::RevalidatePrepared,
        ),
        (
            C::ProgressState,
            None,
            None,
            CommitResultState::RolledBack,
            CommitStage::WriteProgressState,
        ),
        (
            C::ProgressState,
            None,
            Some(R::Cleanup),
            CommitResultState::RolledBackCleanupFailed,
            CommitStage::WriteProgressState,
        ),
        (
            C::ProgressState,
            None,
            Some(R::RemoveNew),
            CommitResultState::RecoveryRequired,
            CommitStage::WriteProgressState,
        ),
        (
            C::CommittedState,
            Some(C::Cleanup),
            None,
            CommitResultState::CommittedCleanupFailed,
            CommitStage::WriteCommittedState,
        ),
        (
            C::CommittedMarkerAfterReplace,
            None,
            None,
            CommitResultState::RecoveryRequired,
            CommitStage::WriteCommittedMarker,
        ),
    ];
    for (point, cleanup_point, rollback_point, expected, stage) in cases {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let plan = CanonicalWritePlan::new().create_document(&document())?;
        let owner = Arc::new(0x8726);
        let hits = Rc::new(Cell::new(0));
        let injected_owner = Arc::clone(&owner);
        let injected_hits = Rc::clone(&hits);
        with_plan(plan, &repo, |plan, permit| -> TestResult {
            let prepared = plan.prepare(&repo, permit)?;
            let expected_id = prepared.transaction_id().clone();
            let result = with_commit_io_factory(
                move |commit, recovery| {
                    if commit == Some(point) {
                        injected_hits.set(injected_hits.get() + 1);
                        Err(cause(&injected_owner, 0x9811))
                    } else if (cleanup_point.is_some() && commit == cleanup_point)
                        || (rollback_point.is_some() && recovery == rollback_point)
                    {
                        injected_hits.set(injected_hits.get() + 1);
                        Err(io::Error::from_raw_os_error(5))
                    } else {
                        Ok(())
                    }
                },
                || prepared.commit(),
            );
            let (diagnostic, original_source) = match &result {
                Ok(outcome) => {
                    let source = match outcome.implementation_original() {
                        CommitOutcome::Committed => panic!("fault must be observed"),
                        CommitOutcome::RolledBack { apply_failure }
                        | CommitOutcome::RolledBackCleanupFailed { apply_failure, .. } => {
                            apply_failure.source.as_ref()
                        }
                        CommitOutcome::CommittedCleanupFailed { failure, .. } => {
                            failure.source.as_ref()
                        }
                    };
                    no_leak(outcome);
                    (outcome.diagnostic(), source)
                }
                Err(error) => {
                    no_leak(error);
                    assert!(!error.to_string().contains(CANARY));
                    assert!(error.source().is_none());
                    (
                        error.diagnostic(),
                        error.fix005_original().failure().source.as_ref(),
                    )
                }
            };
            assert_cause(primary_io(original_source), &owner, 0x9811);
            assert_eq!(diagnostic.state, expected);
            assert_eq!(diagnostic.stage, Some(stage));
            assert_eq!(
                diagnostic.recovery_required,
                matches!(
                    expected,
                    CommitResultState::NotApplied
                        | CommitResultState::RecoveryRequired
                        | CommitResultState::CommittedCleanupFailed
                        | CommitResultState::RolledBackCleanupFailed
                )
            );
            let primary = &diagnostic.failures[0];
            assert_eq!(primary.transaction_id.as_ref(), Some(&expected_id));
            assert_eq!(primary.role, FailureRole::Primary);
            assert_eq!(
                primary.io.as_ref().expect("primary IO").kind,
                io::ErrorKind::PermissionDenied
            );
            let secondary = cleanup_point.is_some() || rollback_point.is_some();
            assert_eq!(diagnostic.failures.len(), if secondary { 2 } else { 1 });
            assert_eq!(hits.get(), if secondary { 2 } else { 1 });
            if secondary {
                let next = &diagnostic.failures[1];
                assert_eq!(
                    next.role,
                    if expected == CommitResultState::RecoveryRequired {
                        FailureRole::Rollback
                    } else {
                        FailureRole::Cleanup
                    }
                );
                assert_eq!(next.io.as_ref().expect("secondary IO").os_code, Some(5));
                assert_eq!(next.transaction_id.as_ref(), Some(&expected_id));
                let raw_secondary = match &result {
                    Ok(outcome) => match outcome.implementation_original() {
                        CommitOutcome::RolledBackCleanupFailed {
                            cleanup_failure, ..
                        } => &cleanup_failure.failure().source,
                        CommitOutcome::CommittedCleanupFailed {
                            cleanup_failure: Some(failure),
                            ..
                        } => primary_io(&failure.source),
                        _ => panic!("expected retained cleanup"),
                    },
                    Err(error) => {
                        &error
                            .fix005_original()
                            .rollback_failure()
                            .expect("retained rollback")
                            .failure()
                            .source
                    }
                };
                assert_eq!(raw_secondary.raw_os_error(), Some(5));
            }
            if point == C::CommittedMarkerAfterReplace {
                assert_eq!(
                    primary.atomic_outcome,
                    Some(SaveOutcome::AppliedDurabilityUncertain)
                );
                assert!(primary.atomic_stage.is_some());
            }
            no_leak(&diagnostic);
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn actual_owned_rename_os_failure_keeps_owned_stage_and_code() -> TestResult {
    let fixture = Fixture::new();
    fixture.write(
        ArtifactSourceId::Document(document_id()),
        &raw(&document_value()),
    );
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let source = repo.load_document(document_id())?;
    let mut changed = document_value();
    changed["name"] = json!("changed");
    let candidate = artifact::decode_document(&raw(&changed))?;
    let plan = CanonicalWritePlan::new().replace_document(&candidate, source.source())?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        // 실제 Win32 공유 모드로 target의 rename만 막는다. I/O 오류 값을 주입하지 않는다.
        let held = fs::OpenOptions::new()
            .read(true)
            .share_mode(3)
            .open(fixture.path(ArtifactSourceId::Document(document_id())))?;
        let result = prepared.commit()?;
        assert_eq!(result.result_state(), CommitResultState::RolledBack);
        let detail = result.diagnostic();
        let io = detail.failures[0].io.as_ref().expect("owned IO");
        assert_eq!(io.os_code, Some(5));
        assert!(io
            .context
            .iter()
            .any(|c| matches!(c, IoContext::Owned(stage) if format!("{stage:?}") == "Rename")));
        assert_eq!(
            detail.failures[0].target,
            Some(ArtifactSourceId::Document(document_id()))
        );
        no_leak(&result);
        drop(held);
        Ok(())
    })?;
    assert!(repo.reread_matches(source.source())?);
    Ok(())
}

#[test]
fn journal_parse_and_nested_cleanup_recovery_project_actual_context() -> TestResult {
    for cleanup in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let ready = runtime.ready()?;
        let repo = ArtifactRepository::new(&ready)?;
        let plan = CanonicalWritePlan::new().create_document(&document())?;
        with_plan(plan, &repo, |plan, permit| -> TestResult {
            let prepared = plan.prepare(&repo, permit)?;
            let directory = prepared.inner_for_test().transaction_directory().to_owned();
            if !cleanup {
                fs::write(
                    directory.join("manifest.json"),
                    format!("{{\"schemaVersion\":2,\"private\":\"{CANARY}\",\"operations\":["),
                )?;
                let error = prepared.commit().unwrap_err();
                assert_eq!(error.result_state(), CommitResultState::NotApplied);
                let diagnostic = error.diagnostic();
                let io = diagnostic.failures[0].io.as_ref().expect("journal IO");
                let json = io
                    .context
                    .iter()
                    .find_map(|c| match c {
                        IoContext::Journal(journal) => journal.json.as_ref(),
                        _ => None,
                    })
                    .expect("journal JSON context");
                assert_eq!(json.category, "Eof");
                assert_eq!(json.line, 1);
                assert!(json.column > 0);
                no_leak(&error);
                assert!(error.source().is_none());
            } else {
                let blocker = Rc::new(RefCell::new(None));
                let held = Rc::clone(&blocker);
                let outcome = with_commit_io_factory(
                    move |point, _| {
                        if point == Some(CommitTestPoint::Cleanup) {
                            // cleanup의 실제 file 삭제를 공유 모드로 막아 RecoveryError -> io::Error를 통과한다.
                            *held.borrow_mut() = Some(
                                fs::OpenOptions::new()
                                    .read(true)
                                    .share_mode(3)
                                    .open(directory.join("manifest.json"))?,
                            );
                        }
                        Ok(())
                    },
                    || prepared.commit(),
                )?;
                assert_eq!(
                    outcome.result_state(),
                    CommitResultState::CommittedCleanupFailed
                );
                let diagnostic = outcome.diagnostic();
                let io = diagnostic.failures[0].io.as_ref().expect("cleanup IO");
                assert_eq!(io.os_code, Some(32));
                assert!(io.context.iter().any(|c| matches!(c, IoContext::Recovery { state: crate::data::transaction::RecoveryResultState::RolledBackCleanupFailed,
                    stage: crate::data::transaction::RecoveryStage::CleanupTransaction, transaction_id: Some(id), target: None }
                    if Some(id) == diagnostic.failures[0].transaction_id.as_ref())));
                no_leak(&outcome);
                blocker.borrow_mut().take();
            }
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn completed_cleanup_preserves_secondary_custom_owner_without_string_conversion() -> TestResult {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let plan = CanonicalWritePlan::new().create_document(&document())?;
    let owner = Arc::new(0x8726);
    let injected = Arc::clone(&owner);
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        let outcome = with_commit_io_factory(
            move |commit, recovery| {
                if commit == Some(CommitTestPoint::ProgressState) {
                    Err(io::Error::from_raw_os_error(5))
                } else if recovery == Some(RecoveryTestPoint::Cleanup) {
                    Err(cause(&injected, 0x9172))
                } else {
                    Ok(())
                }
            },
            || prepared.commit(),
        )?;
        assert_eq!(
            outcome.result_state(),
            CommitResultState::RolledBackCleanupFailed
        );
        let CommitOutcome::RolledBackCleanupFailed {
            apply_failure,
            cleanup_failure,
        } = outcome.implementation_original()
        else {
            panic!("expected retained apply and cleanup")
        };
        assert_eq!(primary_io(&apply_failure.source).raw_os_error(), Some(5));
        assert_cause(&cleanup_failure.failure().source, &owner, 0x9172);
        let detail = outcome.diagnostic();
        assert_eq!(detail.failures.len(), 2);
        assert_eq!(
            detail.failures[0].io.as_ref().expect("primary").os_code,
            Some(5)
        );
        assert_eq!(detail.failures[1].role, FailureRole::Cleanup);
        assert_eq!(
            detail.failures[1]
                .io
                .as_ref()
                .expect("typed secondary")
                .kind,
            io::ErrorKind::PermissionDenied
        );
        no_leak(&outcome);
        Ok(())
    })?;
    Ok(())
}

#[test]
fn actual_atomic_state_and_nested_cleanup_failure_keep_both_atomic_outcomes() -> TestResult {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let ready = runtime.ready()?;
    let repo = ArtifactRepository::new(&ready)?;
    let plan = CanonicalWritePlan::new().create_document(&document())?;
    with_plan(plan, &repo, |plan, permit| -> TestResult {
        let prepared = plan.prepare(&repo, permit)?;
        let state = prepared
            .inner_for_test()
            .transaction_directory()
            .join("state.json");
        let blocker = Rc::new(RefCell::new(None));
        let held = Rc::clone(&blocker);
        let outcome = with_commit_io_factory(
            move |point, _| {
                if point == Some(CommitTestPoint::CommittedState) {
                    *held.borrow_mut() = Some(
                        fs::OpenOptions::new()
                            .read(true)
                            .share_mode(3)
                            .open(&state)?,
                    );
                }
                Ok(())
            },
            || prepared.commit(),
        )?;
        assert_eq!(
            outcome.result_state(),
            CommitResultState::CommittedCleanupFailed
        );
        let detail = outcome.diagnostic();
        assert_eq!(detail.failures.len(), 2);
        let primary = &detail.failures[0];
        assert_eq!(primary.atomic_outcome, Some(SaveOutcome::NotApplied));
        assert_eq!(
            primary.atomic_stage,
            Some(crate::data::atomic_file::AtomicWriteStage::ReplaceTarget)
        );
        assert_eq!(
            primary.io.as_ref().expect("state atomic IO").os_code,
            Some(5)
        );
        let secondary = detail.failures[1].io.as_ref().expect("cleanup atomic IO");
        assert_eq!(secondary.os_code, Some(5));
        assert!(secondary.context.iter().any(|context| matches!(
            context,
            IoContext::Atomic {
                stage: crate::data::atomic_file::AtomicWriteStage::ReplaceTarget,
                outcome: SaveOutcome::NotApplied,
                cleanup: None,
            }
        )));
        let CommitOutcome::CommittedCleanupFailed {
            failure,
            cleanup_failure: Some(cleanup),
        } = outcome.implementation_original()
        else {
            panic!("expected retained state and cleanup failures")
        };
        assert_eq!(primary_io(&failure.source).raw_os_error(), Some(5));
        let original_cleanup = primary_io(&cleanup.source)
            .get_ref()
            .and_then(|source| source.downcast_ref::<crate::data::transaction::RecoveryError>())
            .expect("retained nested recovery");
        let original_save = original_cleanup
            .failure()
            .source
            .get_ref()
            .and_then(|source| source.downcast_ref::<crate::data::atomic_file::SaveError>())
            .expect("retained nested atomic save");
        let crate::data::atomic_file::SaveError::AtomicWrite(original) = original_save else {
            panic!("expected atomic IO")
        };
        assert_eq!(original.source.raw_os_error(), Some(5));
        no_leak(&outcome);
        blocker.borrow_mut().take();
        Ok(())
    })?;
    Ok(())
}
