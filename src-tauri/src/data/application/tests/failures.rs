use super::*;
use crate::data::transaction::{
    test_support::{with_commit_failures, RecoveryTestPoint},
    CommitResultState, CommitStage,
};

#[test]
fn provider_loss_at_issue_prepare_and_apply_preserves_body_and_session_results() {
    for fail_at in 1..=5 {
        let fixture = Fixture::new();
        let old = artifact::encode_template(&template("old")).unwrap();
        fs::write(fixture.path(tid()), &old).unwrap();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let provider = Provider::new();
        let mut session = Session::new(provider.clone());
        session
            .begin_edit(
                runtime.project_fingerprint(),
                targets.session_targets().to_vec(),
            )
            .unwrap();
        provider.reset(fail_at);
        let mut input = template(CANARY);
        let expected = artifact::encode_template(&input).unwrap();
        let mut calls = 0;
        let (result, prepare, _) = observe(|| {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut input,
                &mut |repo: &ArtifactRepository<'_, '_>, input: &mut TemplateArtifact| {
                    calls += 1;
                    replace(repo, input)
                },
            )
        });
        assert_eq!(
            session.snapshot().state(),
            EditSessionState::LockLost,
            "fail at {fail_at}: {result:?}"
        );
        assert_eq!(
            result.diagnostic().permit.unwrap().lock_category,
            Some(LockErrorCategory::LockLost)
        );
        assert_eq!(provider.validations.load(Ordering::SeqCst), fail_at);
        assert_eq!(calls, usize::from(fail_at > 1));
        assert_eq!(prepare.calls, usize::from(fail_at > 1));
        assert_eq!(prepare.allocations, usize::from(fail_at > 2));
        assert_eq!(artifact::encode_template(&input).unwrap(), expected);
        assert_eq!(fs::read(fixture.path(tid())).unwrap(), old);
        if fail_at == 1 {
            assert!(result.body().is_none());
            assert!(result.result.is_err());
        } else {
            let wrapper = result.result.as_ref().unwrap();
            assert!(wrapper.validation_failure().is_some());
            assert!(wrapper.operation_result().is_ok());
        }
        if fail_at == 3 || fail_at == 4 {
            assert_eq!(result.diagnostic().disk, DiskState::NotApplied);
            assert_eq!(
                result.diagnostic().artifact_commit.unwrap().stage,
                Some(if fail_at == 3 {
                    CommitStage::ValidateCommitEntryLock
                } else {
                    CommitStage::ValidateOperationLock
                })
            );
            assert_eq!(runtime.snapshot().state, RuntimeState::Pending);
            assert_eq!(journals(&fixture), 1);
            runtime.recover().unwrap();
        } else if fail_at == 5 {
            assert_eq!(result.diagnostic().disk, DiskState::RolledBack);
            assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
            assert_eq!(journals(&fixture), 0);
        } else {
            assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
            assert_eq!(journals(&fixture), 0);
        }
        assert!(session.retained_failure().is_some());
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn prepare_primary_and_cleanup_failures_set_gate_only_for_residuals() {
    for cleanup in [false, true] {
        let fixture = Fixture::new();
        let old = artifact::encode_template(&template("old")).unwrap();
        fs::write(fixture.path(tid()), &old).unwrap();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut input = template(CANARY);
        let (result, counts) = with_canonical_prepare_hooks(
            move |point, _| {
                if point == PrepareFailPoint::StagedWrite
                    || (cleanup && point == PrepareFailPoint::Cleanup)
                {
                    Err(io::Error::from_raw_os_error(
                        if point == PrepareFailPoint::Cleanup {
                            32
                        } else {
                            5
                        },
                    ))
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
                    &mut replace,
                )
            },
        );
        assert_eq!((counts.calls, counts.allocations), (1, 1));
        assert_eq!(result.diagnostic().disk, DiskState::NotApplied);
        let detail = result.diagnostic().artifact_write.unwrap();
        assert_eq!(detail.io.unwrap().os_code, Some(5));
        assert_eq!(detail.cleanup.is_some(), cleanup);
        assert_eq!(fs::read(fixture.path(tid())).unwrap(), old);
        assert_eq!(journals(&fixture), usize::from(cleanup));
        assert_eq!(
            runtime.snapshot().state,
            if cleanup {
                RuntimeState::Pending
            } else {
                RuntimeState::Ready
            }
        );
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        if cleanup {
            assert!(runtime.ready().is_err());
            runtime.recover().unwrap();
            assert_eq!(journals(&fixture), 0);
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn commit_result_matrix_preserves_disk_journal_and_requires_explicit_recovery() {
    let cases = [
        (
            CommitTestPoint::ManifestRevalidation,
            None,
            CommitResultState::NotApplied,
            DiskState::NotApplied,
            true,
        ),
        (
            CommitTestPoint::TargetSync,
            None,
            CommitResultState::RolledBack,
            DiskState::RolledBack,
            false,
        ),
        (
            CommitTestPoint::TargetSync,
            Some(RecoveryTestPoint::Cleanup),
            CommitResultState::RolledBackCleanupFailed,
            DiskState::RolledBack,
            true,
        ),
        (
            CommitTestPoint::Cleanup,
            None,
            CommitResultState::CommittedCleanupFailed,
            DiskState::Committed,
            true,
        ),
        (
            CommitTestPoint::TargetSync,
            Some(RecoveryTestPoint::BackupVerify),
            CommitResultState::RecoveryRequired,
            DiskState::Uncertain,
            true,
        ),
    ];
    for (point, recovery, state, disk, gate) in cases {
        let fixture = Fixture::new();
        let old = artifact::encode_template(&template("old")).unwrap();
        fs::write(fixture.path(tid()), &old).unwrap();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut input = template(CANARY);
        let new = artifact::encode_template(&input).unwrap();
        let mut calls = 0;
        let (result, counts, commits) = with_commit_failures(Some(point), recovery, || {
            observe(|| {
                run(
                    &mut runtime,
                    &mut session,
                    &targets,
                    &mut input,
                    &mut |repo: &ArtifactRepository<'_, '_>, input: &mut TemplateArtifact| {
                        calls += 1;
                        replace(repo, input)
                    },
                )
            })
        });
        assert_eq!(
            (calls, counts.calls, counts.allocations, commits),
            (1, 1, 1, 1)
        );
        assert_eq!(
            result.diagnostic().artifact_commit.unwrap().state,
            state,
            "{result:?}"
        );
        assert_eq!(result.diagnostic().disk, disk);
        assert_eq!(result.diagnostic().recovery_required, gate);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        assert_eq!(journals(&fixture), usize::from(gate));
        assert_eq!(
            runtime.snapshot().state,
            if gate {
                RuntimeState::Pending
            } else {
                RuntimeState::Ready
            }
        );
        if disk == DiskState::Committed {
            assert_eq!(fs::read(fixture.path(tid())).unwrap(), new);
        } else if disk != DiskState::Uncertain {
            assert_eq!(fs::read(fixture.path(tid())).unwrap(), old);
        }
        if gate {
            assert!(runtime.ready().is_err());
            let before = inventory(&fixture.root);
            let mut next_calls = 0;
            let (next, prepare, commits) = observe(|| {
                run(
                    &mut runtime,
                    &mut session,
                    &targets,
                    &mut (),
                    &mut |_: &ArtifactRepository<'_, '_>,
                          _: &mut ()|
                     -> Result<_, BuildError<()>> {
                        next_calls += 1;
                        Ok(WriteDecision::NoWrite(()))
                    },
                )
            });
            assert_eq!(
                (next_calls, prepare.calls, prepare.allocations, commits),
                (0, 0, 0, 0)
            );
            assert_eq!(
                next.diagnostic().category,
                Some(ApplicationCategory::RuntimeRejected)
            );
            assert_eq!(inventory(&fixture.root), before);
            runtime.recover().unwrap();
            assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
            assert_eq!(journals(&fixture), 0);
            assert_eq!(calls, 1);
            let expected = if disk == DiskState::Committed {
                &new
            } else {
                &old
            };
            assert_eq!(&fs::read(fixture.path(tid())).unwrap(), expected);
            let (next, prepare, commits) = observe(|| {
                run(
                    &mut runtime,
                    &mut session,
                    &targets,
                    &mut (),
                    &mut |repo: &ArtifactRepository<'_, '_>,
                          _: &mut ()|
                     -> Result<_, BuildError<()>> {
                        repo.load_template(template_id())?;
                        next_calls += 1;
                        Ok(WriteDecision::NoWrite(()))
                    },
                )
            });
            assert_eq!(next.diagnostic().disk, DiskState::NoWrite);
            assert_eq!((next_calls, prepare.calls, commits), (1, 0, 0));
            assert_eq!(calls, 1);
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
