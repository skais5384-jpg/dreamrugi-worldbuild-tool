use super::*;
use crate::data::transaction::{
    artifact_diagnostics::FailureRole, test_support::RecoveryTestPoint, CommitFailureSource,
    CommitOutcome, PrepareStage,
};
use std::{error::Error, fmt};

#[derive(Debug)]
struct PrivateCause {
    owner: Arc<u64>,
    secret: &'static str,
}
impl fmt::Display for PrivateCause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.secret)
    }
}
impl Error for PrivateCause {}
fn cause(owner: &Arc<u64>) -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        PrivateCause {
            owner: Arc::clone(owner),
            secret: CANARY,
        },
    )
}
fn assert_cause(error: &io::Error, owner: &Arc<u64>) {
    let cause = error
        .get_ref()
        .unwrap()
        .downcast_ref::<PrivateCause>()
        .unwrap();
    assert!(Arc::ptr_eq(&cause.owner, owner));
    assert_eq!(cause.secret, CANARY);
}
fn safe(error: &impl Error) {
    let mut rendered = format!("{error:?} {error}");
    let mut source = error.source();
    while let Some(next) = source {
        rendered.push_str(&format!("{next:?} {next}"));
        source = next.source();
    }
    for secret in [
        CANARY,
        "credential=secret",
        "private/body.json",
        "token=private",
    ] {
        assert!(!rendered.contains(secret));
    }
}

#[test]
fn real_prepare_original_and_cleanup_owners_remain_private_after_projection() {
    let fixture = Fixture::new();
    let mut runtime = fixture.runtime();
    let targets = ExactWriteTargets::new([tid()]).unwrap();
    let mut session = session(&runtime, &targets);
    let owner = Arc::new(567);
    let injected = Arc::clone(&owner);
    let mut input = template(CANARY);
    let (result, counts) = with_canonical_prepare_hooks(
        move |point, _| {
            if point == PrepareFailPoint::StagedWrite {
                Err(cause(&injected))
            } else if point == PrepareFailPoint::Cleanup {
                Err(io::Error::from_raw_os_error(5))
            } else {
                Ok(())
            }
        },
        || {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut input,
                &mut |_: &ArtifactRepository<'_, '_>,
                      input: &mut TemplateArtifact|
                 -> Result<_, BuildError<()>> {
                    Ok(WriteDecision::Write {
                        plan: CanonicalWritePlan::new().create_template(input)?,
                        value: (),
                    })
                },
            )
        },
    );
    assert_eq!((counts.calls, counts.allocations), (1, 1));
    let BodyOutcome::Write {
        transaction: TransactionResult::PrepareFailed(error),
        ..
    } = result.body().unwrap()
    else {
        panic!("{result:?}")
    };
    let original = error.implementation_original_prepare().unwrap();
    let pointer = original.implementation_original_io().unwrap() as *const _;
    for _ in 0..3 {
        let diag = result.diagnostic();
        assert_eq!(
            diag.artifact_write.as_ref().unwrap().prepare_stage,
            Some(PrepareStage::WriteStaged)
        );
        let failures = &diag.artifact_write.as_ref().unwrap().failures;
        assert_eq!(failures[0].role, FailureRole::Primary);
        assert_eq!(failures[1].role, FailureRole::Cleanup);
        assert_eq!(failures[0].target, Some(tid()));
        assert_eq!(failures[1].io.as_ref().unwrap().os_code, Some(5));
        assert!(!format!("{result:?} {diag:?}").contains(CANARY));
        assert!(!diag.next_action().is_empty());
        safe(error);
        assert_eq!(
            original.implementation_original_io().unwrap() as *const _,
            pointer
        );
        assert_cause(original.implementation_original_io().unwrap(), &owner);
    }
    assert_eq!(original.cleanup_error().unwrap().raw_os_error(), Some(5));
    assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
    runtime.recover().unwrap();
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn real_rollback_cleanup_and_uncertain_atomic_results_keep_original_causes() {
    for atomic in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut input = template(CANARY);
        let owner = Arc::new(923);
        let injected = Arc::clone(&owner);
        let result = with_commit_io_factory(
            move |commit, recovery| {
                if commit
                    == Some(if atomic {
                        CommitTestPoint::CommittedMarkerAfterReplace
                    } else {
                        CommitTestPoint::TargetSync
                    })
                {
                    Err(cause(&injected))
                } else if !atomic && recovery == Some(RecoveryTestPoint::Cleanup) {
                    Err(io::Error::from_raw_os_error(5))
                } else {
                    Ok(())
                }
            },
            || {
                run(
                    &mut runtime,
                    &mut session,
                    &targets,
                    &mut input,
                    &mut |_: &ArtifactRepository<'_, '_>,
                          input: &mut TemplateArtifact|
                     -> Result<_, BuildError<()>> {
                        Ok(WriteDecision::Write {
                            plan: CanonicalWritePlan::new().create_template(input)?,
                            value: (),
                        })
                    },
                )
            },
        );
        let BodyOutcome::Write {
            transaction: TransactionResult::Commit(commit),
            ..
        } = result.body().unwrap()
        else {
            panic!("{result:?}")
        };
        let diag = result.diagnostic();
        assert!(diag.recovery_required);
        assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
        assert!(!format!("{result:?} {diag:?}").contains(CANARY));
        if atomic {
            let error = commit.as_ref().unwrap_err();
            safe(error);
            let original = error.fix005_original();
            let CommitFailureSource::AtomicSave(crate::data::atomic_file::SaveError::AtomicWrite(
                atomic_error,
            )) = original.failure().source.as_ref()
            else {
                panic!("atomic source")
            };
            assert_eq!(
                atomic_error.outcome,
                crate::data::atomic_file::SaveOutcome::AppliedDurabilityUncertain
            );
            assert_cause(&atomic_error.source, &owner);
            assert_eq!(diag.disk, DiskState::Uncertain);
            assert!(format!("{:?}", diag.artifact_commit).contains("AppliedDurabilityUncertain"));
        } else {
            let outcome = commit.as_ref().unwrap();
            let CommitOutcome::RolledBackCleanupFailed {
                apply_failure,
                cleanup_failure,
            } = outcome.implementation_original()
            else {
                panic!("cleanup outcome")
            };
            let CommitFailureSource::Io(error) = apply_failure.source.as_ref() else {
                panic!("IO source")
            };
            assert_cause(error, &owner);
            assert_eq!(cleanup_failure.failure().io_cause().raw_os_error(), Some(5));
            assert_eq!(diag.disk, DiskState::RolledBack);
            let failures = &diag.artifact_commit.as_ref().unwrap().failures;
            assert_eq!(failures[0].role, FailureRole::Primary);
            assert_eq!(failures[1].role, FailureRole::Cleanup);
        }
        runtime.recover().unwrap();
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
