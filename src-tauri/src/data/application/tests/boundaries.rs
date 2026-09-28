use super::*;

#[test]
fn exact_targets_sort_once_and_reject_empty_duplicate() {
    let forward = ExactWriteTargets::new([tid(), did()]).unwrap();
    let reverse = ExactWriteTargets::new([did(), tid()]).unwrap();
    assert_eq!(forward, reverse);
    assert!(forward.session_targets()[0]
        .as_str()
        .starts_with("documents/"));
    assert_eq!(
        ExactWriteTargets::new([]).unwrap_err().category(),
        ApplicationCategory::EmptyTargets
    );
    assert_eq!(
        ExactWriteTargets::new([tid(), did(), tid()])
            .unwrap_err()
            .category(),
        ApplicationCategory::DuplicateTarget
    );
}

#[test]
fn create_control_and_two_entry_plan_use_one_prepare_commit() {
    for mixed in [false, true] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new(if mixed {
            vec![tid(), did()]
        } else {
            vec![tid()]
        })
        .unwrap();
        let provider = Provider::new();
        let mut session = Session::new(provider.clone());
        session
            .begin_edit(
                runtime.project_fingerprint(),
                targets.session_targets().to_vec(),
            )
            .unwrap();
        provider.reset(0);
        let mut candidates = (template(CANARY), document());
        let expected_t = artifact::encode_template(&candidates.0).unwrap();
        let expected_d = artifact::encode_document(&candidates.1).unwrap();
        let mut body_calls = 0;
        let (result, prepare, commits) = observe(|| {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut candidates,
                &mut |_: &ArtifactRepository<'_, '_>,
                      input: &mut (TemplateArtifact, DocumentArtifact)|
                 -> Result<_, BuildError<()>> {
                    body_calls += 1;
                    let plan = CanonicalWritePlan::new().create_template(&input.0)?;
                    let plan = if mixed {
                        plan.create_document(&input.1)?
                    } else {
                        plan
                    };
                    Ok(WriteDecision::Write { plan, value: () })
                },
            )
        });
        assert_eq!(result.diagnostic().disk, DiskState::Committed, "{result:?}");
        assert_eq!(
            (body_calls, prepare.calls, prepare.allocations, commits),
            (1, 1, 1, 1)
        );
        // 1 target: issue/namespace/prepare/commit-entry/operation/before-marker 각 1회.
        // 2 targets: validate_all은 매번 전체 집합을 검증하며 두 operation 각각 한 번이다.
        assert_eq!(
            provider.validations.load(Ordering::SeqCst),
            if mixed { 14 } else { 6 }
        );
        assert_eq!(fs::read(fixture.path(tid())).unwrap(), expected_t);
        assert_eq!(
            artifact::decode_template(&expected_t).unwrap(),
            candidates.0
        );
        if mixed {
            assert_eq!(fs::read(fixture.path(did())).unwrap(), expected_d);
            assert_eq!(
                artifact::decode_document(&expected_d).unwrap(),
                candidates.1
            );
        }
        assert_eq!(journals(&fixture), 0);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn g6_namespace_permit_refusal_preserves_ticket_before_any_directory_or_prepare() {
    use crate::data::application::templates::{
        self, CreateTemplateInput, CreationState, TemplateWriteContext,
    };
    let fixture = Fixture::new();
    fs::remove_dir(fixture.root.join("templates")).unwrap();
    let mut runtime = fixture.runtime();
    let input = CreateTemplateInput {
        name: CANARY.into(),
        presentation_token: None,
        timestamp_utc: TIME.into(),
    };
    let mut ticket = templates::prepare_create_template(&mut runtime, &input).unwrap();
    let ptr = ticket.candidate() as *const _;
    let id = ticket.template_id();
    let provider = Provider::new();
    let mut session = Session::new(provider.clone());
    session
        .begin_edit(
            runtime.project_fingerprint(),
            ticket.session_targets().to_vec(),
        )
        .unwrap();
    let snapshot = session.snapshot();
    provider.reset(2);
    let (result, counts, commits) = observe(|| {
        templates::create_template(
            &mut runtime,
            &mut session,
            TemplateWriteContext {
                project: snapshot.project_fingerprint().unwrap(),
                session: snapshot.session_id().unwrap(),
            },
            &mut ticket,
        )
    });
    assert_eq!(provider.validations.load(Ordering::SeqCst), 2);
    assert_eq!((counts.calls, counts.allocations, commits), (0, 0, 0));
    assert_eq!(result.diagnostic().disk, DiskState::NotApplied);
    assert_eq!(session.snapshot().state(), EditSessionState::LockLost);
    assert!(result
        .result
        .as_ref()
        .unwrap()
        .validation_failure()
        .is_some());
    assert_eq!(ticket.template_id(), id);
    assert_eq!(ticket.candidate() as *const _, ptr);
    assert_eq!(ticket.state(), CreationState::Uncommitted);
    assert!(!fixture.root.join("templates").exists());
    assert_eq!(journals(&fixture), 0);
    assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
    session.end_edit().unwrap();
    runtime.close().unwrap();
}

#[test]
fn all_plan_targets_are_checked_before_prepare_and_owner_survives() {
    for case in ["missing", "extra", "empty", "second_build"] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new(if case == "extra" {
            vec![tid()]
        } else {
            vec![did(), tid()]
        })
        .unwrap();
        let mut session = session(&runtime, &targets);
        let before = inventory(&fixture.root);
        let mut input = (template(CANARY), document());
        let mut steps = 0;
        let (result, prepare, commits) = observe(|| {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut input,
                &mut |_: &ArtifactRepository<'_, '_>,
                      input: &mut (TemplateArtifact, DocumentArtifact)|
                 -> Result<_, BuildError<String>> {
                    steps += 1;
                    let plan = if case == "empty" {
                        CanonicalWritePlan::new()
                    } else {
                        CanonicalWritePlan::new().create_template(&input.0)?
                    };
                    if case == "second_build" {
                        steps += 1;
                        return Err(BuildError::domain(CANARY.to_owned()));
                    }
                    let plan = if case == "extra" {
                        plan.create_document(&input.1)?
                    } else {
                        plan
                    };
                    Ok(WriteDecision::Write {
                        plan,
                        value: Box::new(CANARY.to_owned()),
                    })
                },
            )
        });
        assert_eq!((prepare.calls, prepare.allocations, commits), (0, 0, 0));
        assert_eq!(inventory(&fixture.root), before);
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        if case == "second_build" {
            assert_eq!(steps, 2);
            let BodyOutcome::Rejected(error) = result.body().unwrap() else {
                panic!("{result:?}")
            };
            assert_eq!(error.domain_cause().unwrap(), CANARY);
        } else {
            assert_eq!(steps, 1);
            assert_eq!(
                result.diagnostic().category,
                Some(ApplicationCategory::PlanTargetsMismatch)
            );
            let BodyOutcome::Write { value, .. } = result.body().unwrap() else {
                panic!("owner absent")
            };
            assert_eq!(value.as_str(), CANARY);
        }
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn structural_mismatch_preserves_unrelated_session_and_nonclone_input() {
    struct Input {
        secret: String,
    }
    for case in ["runtime", "session", "missing", "extra", "state"] {
        let fixture = Fixture::new();
        let other = Fixture::new();
        let mut runtime = fixture.runtime();
        let mut other_runtime = other.runtime();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut other_session = super::session(&runtime, &targets);
        let snapshot = session.snapshot();
        let request_targets = ExactWriteTargets::new(if case == "missing" {
            vec![did()]
        } else if case == "extra" {
            vec![tid(), did()]
        } else {
            vec![tid()]
        })
        .unwrap();
        if case == "state" {
            session.preserve_for_recovery(()).unwrap();
        }
        let before_state = session.snapshot().state();
        let before = inventory(&fixture.root);
        let mut input = Input {
            secret: CANARY.to_owned(),
        };
        let owner = input.secret.as_ptr();
        let mut calls = 0;
        let (result, prepare, commits) = observe(|| {
            execute_write_operation(
                if case == "runtime" {
                    &mut other_runtime
                } else {
                    &mut runtime
                },
                if case == "session" {
                    &mut other_session
                } else {
                    &mut session
                },
                WriteRequest {
                    project: snapshot.project_fingerprint().unwrap(),
                    session: snapshot.session_id().unwrap(),
                    targets: &request_targets,
                },
                &mut input,
                &mut |_: &ArtifactRepository<'_, '_>,
                      _: &mut Input|
                 -> Result<WriteDecision<()>, BuildError<()>> {
                    calls += 1;
                    Ok(WriteDecision::NoWrite(()))
                },
            )
        });
        assert_eq!(
            (calls, prepare.calls, prepare.allocations, commits),
            (0, 0, 0, 0)
        );
        let expected = match case {
            "runtime" => ApplicationCategory::ProjectMismatch,
            "session" => ApplicationCategory::SessionMismatch,
            "state" => ApplicationCategory::InvalidSessionState,
            _ => ApplicationCategory::SessionTargetsMismatch,
        };
        assert_eq!(
            result.diagnostic().category,
            Some(expected),
            "{case}: {result:?}"
        );
        assert_eq!(input.secret.as_ptr(), owner);
        assert_eq!(input.secret, CANARY);
        assert_eq!(inventory(&fixture.root), before);
        assert_eq!(session.snapshot().state(), before_state);
        assert_eq!(other_session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        other_session.end_edit().unwrap();
        runtime.close().unwrap();
        other_runtime.close().unwrap();
    }
}

#[test]
fn pending_blocked_and_duplicate_runtime_never_enter_body() {
    for blocked in [false, true] {
        let fixture = Fixture::new();
        let mut runtime =
            ProjectRuntime::acquire(&fixture.root, &fixture.base.join("locks")).unwrap();
        let targets = ExactWriteTargets::new([tid()]).unwrap();
        let mut session = session(&runtime, &targets);
        assert!(ProjectRuntime::acquire(&fixture.root, &fixture.base.join("locks")).is_err());
        if blocked {
            let journal = fixture.root.join(".worldbuild/transactions/unrecognized");
            fs::create_dir_all(journal).unwrap();
            assert!(runtime.recover().is_err());
            assert_eq!(runtime.snapshot().state, RuntimeState::Blocked);
        }
        let before = inventory(&fixture.root);
        let mut calls = 0;
        let (result, prepare, commits) = observe(|| {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut (),
                &mut |_: &ArtifactRepository<'_, '_>, _: &mut ()| -> Result<_, BuildError<()>> {
                    calls += 1;
                    Ok(WriteDecision::NoWrite(()))
                },
            )
        });
        assert_eq!(
            (calls, prepare.calls, prepare.allocations, commits),
            (0, 0, 0, 0)
        );
        assert_eq!(
            result.diagnostic().category,
            Some(ApplicationCategory::RuntimeRejected)
        );
        assert_eq!(
            runtime.snapshot().state,
            if blocked {
                RuntimeState::Blocked
            } else {
                RuntimeState::Pending
            }
        );
        assert_eq!(inventory(&fixture.root), before);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn read_dependency_is_reread_inside_operation_and_excluded_from_targets() {
    for stale in [false, true] {
        let fixture = Fixture::new();
        fs::write(
            fixture.path(tid()),
            artifact::encode_template(&template("observed")).unwrap(),
        )
        .unwrap();
        let mut runtime = fixture.runtime();
        let source = {
            let ready = runtime.ready().unwrap();
            let repo = ArtifactRepository::new(&ready).unwrap();
            repo.load_template(template_id()).unwrap().source().clone()
        };
        let targets = ExactWriteTargets::new([did()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut input = document();
        let mut calls = 0;
        let dependency_path = fixture.path(tid());
        let (result, prepare, commits) = observe(|| {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut input,
                &mut |repo: &ArtifactRepository<'_, '_>,
                      candidate: &mut DocumentArtifact|
                 -> Result<_, BuildError<&'static str>> {
                    calls += 1;
                    // 최종 재검사 직전의 외부 변경만 모사한다. 검사 이후 writer 배제의 증거가 아니다.
                    if stale {
                        fs::write(
                            &dependency_path,
                            artifact::encode_template(&template("changed")).unwrap(),
                        )
                        .unwrap();
                    }
                    if !repo.reread_matches(&source)? {
                        return Err(BuildError::domain("StaleDependency"));
                    }
                    let dependency = repo.load_template(template_id())?;
                    assert_eq!(dependency.artifact().template_id(), candidate.template_id());
                    Ok(WriteDecision::Write {
                        plan: CanonicalWritePlan::new().create_document(candidate)?,
                        value: (),
                    })
                },
            )
        });
        assert_eq!(calls, 1);
        assert_eq!(session.snapshot().targets(), targets.session_targets());
        assert_eq!(targets.session_targets().len(), 1);
        if stale {
            assert_eq!((prepare.calls, prepare.allocations, commits), (0, 0, 0));
            assert!(!fixture.path(did()).exists());
            assert_eq!(
                result.diagnostic().category,
                Some(ApplicationCategory::DomainRejected)
            );
            let BodyOutcome::Rejected(error) = result.body().unwrap() else {
                panic!("stale refusal")
            };
            assert_eq!(error.domain_cause(), Some(&"StaleDependency"));
        } else {
            assert_eq!((prepare.calls, prepare.allocations, commits), (1, 1, 1));
            assert_eq!(result.diagnostic().disk, DiskState::Committed);
            assert_eq!(
                fs::read(fixture.path(did())).unwrap(),
                artifact::encode_document(&input).unwrap()
            );
        }
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}

#[test]
fn source_read_failure_and_second_canonical_build_failure_never_allocate() {
    for source_failure in [true, false] {
        let fixture = Fixture::new();
        let mut runtime = fixture.runtime();
        let targets = ExactWriteTargets::new([tid(), did()]).unwrap();
        let mut session = session(&runtime, &targets);
        let mut input = template(CANARY);
        let before = inventory(&fixture.root);
        let mut steps = 0;
        let (result, prepare, commits) = observe(|| {
            run(
                &mut runtime,
                &mut session,
                &targets,
                &mut input,
                &mut |repo: &ArtifactRepository<'_, '_>,
                      input: &mut TemplateArtifact|
                 -> Result<WriteDecision<()>, BuildError<()>> {
                    steps += 1;
                    if source_failure {
                        repo.load_template(template_id())?;
                    }
                    let plan = CanonicalWritePlan::new().create_template(input)?;
                    steps += 1;
                    let plan = plan.create_template(input)?;
                    Ok(WriteDecision::Write { plan, value: () })
                },
            )
        });
        assert_eq!((prepare.calls, prepare.allocations, commits), (0, 0, 0));
        assert_eq!(steps, if source_failure { 1 } else { 2 });
        assert_eq!(inventory(&fixture.root), before);
        if source_failure {
            assert_eq!(
                result.diagnostic().repository.unwrap().category,
                crate::data::repository::RepositoryCategory::NotFound
            );
        } else {
            assert_eq!(
                result.diagnostic().artifact_write.unwrap().category,
                crate::data::repository::ArtifactWriteCategory::DuplicateTarget
            );
        }
        assert_eq!(runtime.snapshot().state, RuntimeState::Ready);
        assert_eq!(session.snapshot().state(), EditSessionState::Editing);
        session.end_edit().unwrap();
        runtime.close().unwrap();
    }
}
